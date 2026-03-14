use chrono::{DateTime, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use p384::SecretKey;
use p384::pkcs8::{EncodePrivateKey, LineEnding, spki::EncodePublicKey};
use rand_core::OsRng;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── Claims ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    /// ServiceAccount name.
    pub sub: String,
    pub namespace: String,
    pub token_name: String,
    pub token_id: Uuid,
    /// Unix timestamp (seconds).
    pub exp: u64,
}

// ── Key pair ──────────────────────────────────────────────────────────────────

pub struct EcKeyPair {
    pub private_key_pem: Vec<u8>,
    pub public_key_pem: Vec<u8>,
}

// ── Errors ────────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum JwtError {
    #[error("JWT error: {0}")]
    Jwt(#[from] jsonwebtoken::errors::Error),
    #[error("Key error: {0}")]
    Key(String),
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Generate a P-384 keypair and return both keys as PEM bytes.
pub fn generate_keypair() -> Result<EcKeyPair, JwtError> {
    let secret_key = SecretKey::random(&mut OsRng);
    let private_key_pem = secret_key
        .to_pkcs8_pem(LineEnding::LF)
        .map_err(|e| JwtError::Key(e.to_string()))?
        .as_bytes()
        .to_vec();
    let public_key_pem = secret_key
        .public_key()
        .to_public_key_pem(LineEnding::LF)
        .map_err(|e| JwtError::Key(e.to_string()))?
        .into_bytes();
    Ok(EcKeyPair { private_key_pem, public_key_pem })
}

/// Create a signed JWT using ES384 (ECDSA with P-384).
///
/// `private_key_pem` must be a PEM-encoded PKCS#8 EC private key.
pub fn create_token(
    service_account: &str,
    namespace: &str,
    token_name: &str,
    token_id: Uuid,
    expires_at: DateTime<Utc>,
    private_key_pem: &[u8],
) -> Result<String, JwtError> {
    let claims = Claims {
        sub: service_account.to_string(),
        namespace: namespace.to_string(),
        token_name: token_name.to_string(),
        token_id,
        exp: expires_at.timestamp() as u64,
    };
    Ok(encode(
        &Header::new(Algorithm::ES384),
        &claims,
        &EncodingKey::from_ec_pem(private_key_pem)?,
    )?)
}

/// Validate an ES384 JWT and return its claims.
///
/// `public_key_pem` must be a PEM-encoded EC public key.
pub fn validate_token(token: &str, public_key_pem: &[u8]) -> Result<Claims, JwtError> {
    let mut validation = Validation::new(Algorithm::ES384);
    validation.validate_exp = true;
    Ok(decode::<Claims>(
        token,
        &DecodingKey::from_ec_pem(public_key_pem)?,
        &validation,
    )
    .map(|data| data.claims)?)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn test_keypair_generation_and_jwt_roundtrip() {
        let keypair = generate_keypair().expect("keypair generation failed");

        let token_id = Uuid::new_v4();
        let expires_at = Utc::now() + Duration::hours(1);

        let token = create_token(
            "superadmin",
            "system",
            "my-token",
            token_id,
            expires_at,
            &keypair.private_key_pem,
        )
        .expect("token creation failed");

        let claims = validate_token(&token, &keypair.public_key_pem)
            .expect("token validation failed");

        assert_eq!(claims.sub, "superadmin");
        assert_eq!(claims.namespace, "system");
        assert_eq!(claims.token_name, "my-token");
        assert_eq!(claims.token_id, token_id);
    }

    #[test]
    fn test_validation_rejects_wrong_key() {
        let keypair_a = generate_keypair().unwrap();
        let keypair_b = generate_keypair().unwrap();

        let token = create_token(
            "superadmin", "system", "tok", Uuid::new_v4(),
            Utc::now() + Duration::hours(1),
            &keypair_a.private_key_pem,
        )
        .unwrap();

        assert!(validate_token(&token, &keypair_b.public_key_pem).is_err());
    }
}
