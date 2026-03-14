use dawnstore_lib::{AllowedScope, GetObjectsFilter, ReturnObject};

use crate::abstractions::{BackendGetObjectsFilter, NewDawnStoreBackend};
use crate::cache::DawnstoreCache;
use crate::error::DawnStoreError;
use crate::rbac::authz_service::is_superadmin;
use crate::rbac::cache::{EffectivePermissions, GrantedScope, Verb};
use crate::rbac::middleware::Claims;

// ── Private helpers ───────────────────────────────────────────────────────────

/// Resolve `kind` to its canonical name through the alias cache.
///
/// Returns [`DawnStoreError::UnknownResourceKind`] when the alias is not
/// registered — callers must register schemas before querying objects of that kind.
async fn resolve_kind(cache: &DawnstoreCache, kind: &str) -> Result<String, DawnStoreError> {
    cache
        .resolve_kind(kind)
        .await
        .ok_or_else(|| DawnStoreError::UnknownResourceKind(kind.to_string()))
}

/// Load the effective permissions for `caller` from the cache, rebuilding from
/// `backend` on a miss.
async fn get_or_load_permissions<B: NewDawnStoreBackend>(
    cache: &DawnstoreCache,
    backend: &B,
    caller: &Claims,
) -> Result<EffectivePermissions, DawnStoreError> {
    if let Some(perms) = cache.get_permissions(&caller.namespace, &caller.sub) {
        return Ok(perms);
    }
    cache.init_permission(backend).await?;
    Ok(cache.get_permissions(&caller.namespace, &caller.sub).unwrap_or_default())
}

/// Compute the RBAC `allowed` constraint to inject into the backend filter.
///
/// - `None` (unauthenticated / superadmin path): unrestricted — backend returns everything.
/// - `Some([])` (authenticated, no Get grants): deny all — backend returns nothing.
/// - `Some([...])`: the caller's effective Get-allowed scopes; the backend restricts
///   its result set to objects that match at least one scope.
///
/// Namespace-scoped grants from the caller's `RoleBinding`s apply only within
/// the SA's own namespace (`caller.namespace`). Global grants (from `GlobalRoleBinding`s)
/// apply across all namespaces (`AllowedScope.namespace = None`).
async fn build_allowed<B: NewDawnStoreBackend>(
    cache: &DawnstoreCache,
    backend: &B,
    caller: Option<&Claims>,
) -> Result<Option<Vec<AllowedScope>>, DawnStoreError> {
    let Some(caller) = caller else {
        return Ok(None); // unauthenticated / superadmin path: unrestricted
    };
    if is_superadmin(caller) {
        return Ok(None);
    }

    let perms = get_or_load_permissions(cache, backend, caller).await?;
    let mut scopes: Vec<AllowedScope> = Vec::new();

    for grant in &perms.namespaced {
        add_get_scopes(grant, Some(caller.namespace.clone()), &mut scopes);
    }
    for grant in &perms.global {
        add_get_scopes(grant, None, &mut scopes);
    }

    Ok(Some(scopes))
}

/// Push one `AllowedScope` per kind into `out` for every Get-granting scope.
fn add_get_scopes(grant: &GrantedScope, namespace: Option<String>, out: &mut Vec<AllowedScope>) {
    if !grant.verbs.contains(&Verb::Get) {
        return;
    }
    for kind in &grant.kinds {
        out.push(AllowedScope {
            namespace: namespace.clone(),
            kind: kind.clone(),
            names: grant.names.clone(),
        });
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Retrieve objects matching `filter`, enforcing RBAC for authenticated callers.
///
/// Steps:
/// 1. If `filter.kind` is set, resolve it through the kind-alias cache.
///    Returns [`DawnStoreError::UnknownResourceKind`] if the alias is not registered.
/// 2. Compute the RBAC `allowed` constraint for `caller`:
///    - `None` (unauthenticated or superadmin): unrestricted — all matching objects returned.
///    - `Some([])` (no Get grants): deny all — returns an empty list without a hard error.
///    - `Some([...])`: restrict to objects the caller may read; injected into the
///      backend filter so the backend can apply the restriction at query time.
/// 3. Delegate to `backend.get_objects()` with the resolved filter + RBAC constraint.
pub async fn get<B: NewDawnStoreBackend>(
    backend: &B,
    cache: &DawnstoreCache,
    caller: Option<&Claims>,
    filter: GetObjectsFilter,
) -> Result<Vec<ReturnObject<serde_json::Value>>, DawnStoreError> {
    // Step 1: resolve kind alias.
    let kind = match filter.kind {
        Some(k) => Some(resolve_kind(cache, &k).await?),
        None => None,
    };

    // Step 2: compute RBAC filter.
    let allowed = build_allowed(cache, backend, caller).await?;

    // Step 3: delegate to backend.
    backend
        .get_objects(&BackendGetObjectsFilter {
            namespace: filter.namespace,
            kind,
            name: filter.name,
            allowed,
        })
        .await
}
