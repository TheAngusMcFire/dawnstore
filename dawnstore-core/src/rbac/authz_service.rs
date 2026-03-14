use super::middleware::Claims;

/// Returns `true` if the caller is the system superadmin.
///
/// The superadmin (`system/serviceaccount/superadmin`) bypasses all
/// authorization checks and is the only identity that may issue tokens.
pub fn is_superadmin(claims: &Claims) -> bool {
    claims.namespace == "system" && claims.sub == "superadmin"
}
