use dawnstore_lib::AllowedScope;

use crate::abstractions::DawnstoreBackend;
use crate::error::DawnStoreError;
use super::cache::{RbacCache, Verb};
use super::constants::{SA_SUPERADMIN, SYSTEM_NAMESPACE};
use super::middleware::Claims;

/// Returns `true` if the caller is the system superadmin.
///
/// The superadmin (`system/serviceaccount/superadmin`) bypasses all
/// authorization checks and is the only identity that may issue tokens.
pub fn is_superadmin(claims: &Claims) -> bool {
    claims.namespace == SYSTEM_NAMESPACE && claims.sub == SA_SUPERADMIN
}

/// Returns `true` if the caller is authorised to perform `verb` on a specific
/// `(namespace, kind, name)` triple.
///
/// Superadmin always returns `true`. For all other identities the RBAC cache is
/// consulted (with a DB fallback on a miss).
pub async fn is_allowed<B: DawnstoreBackend>(
    cache: &RbacCache,
    backend: &B,
    claims: &Claims,
    verb: Verb,
    kind: &str,
    name: &str,
) -> Result<bool, DawnStoreError> {
    if is_superadmin(claims) {
        return Ok(true);
    }
    let perms = cache.get_or_load(backend, &claims.namespace, &claims.sub).await?;
    // Try any api_version wildcard since we don't track api_version per-request yet.
    Ok(perms.is_allowed(verb, "*", kind, name) || perms.is_allowed(verb, &claims.namespace, kind, name))
}

/// Returns the set of `(namespace, kind[, names])` scopes the caller is
/// permitted to perform `verb` on, or `None` if the caller is unrestricted
/// (superadmin).
///
/// An empty `Some(vec)` means the caller has no access to anything.
/// The returned value can be injected directly into `GetObjectsFilter::allowed`.
pub async fn allowed_scopes<B: DawnstoreBackend>(
    cache: &RbacCache,
    backend: &B,
    claims: &Claims,
    verb: Verb,
) -> Result<Option<Vec<AllowedScope>>, DawnStoreError> {
    if is_superadmin(claims) {
        return Ok(None);
    }
    let perms = cache.get_or_load(backend, &claims.namespace, &claims.sub).await?;
    let mut scopes: Vec<AllowedScope> = Vec::new();

    // Namespace-scoped grants: the SA's namespace is the effective namespace.
    for grant in &perms.namespaced {
        if !grant.verbs.contains(&verb) {
            continue;
        }
        for kind in &grant.kinds {
            scopes.push(AllowedScope {
                namespace: Some(claims.namespace.clone()),
                kind: kind.clone(),
                names: grant.names.clone(),
            });
        }
    }

    // Global grants: apply across all namespaces (namespace = None).
    for grant in &perms.global {
        if !grant.verbs.contains(&verb) {
            continue;
        }
        for kind in &grant.kinds {
            scopes.push(AllowedScope {
                namespace: None,
                kind: kind.clone(),
                names: grant.names.clone(),
            });
        }
    }

    Ok(Some(scopes))
}
