use dawnstore_lib::DeleteObject;

use crate::abstractions::DawnstoreBackend;
use crate::cache::DawnstoreCache;
use crate::error::DawnStoreError;
use crate::cache::{EffectivePermissions, Verb, is_superadmin};
use crate::rbac::constants::{
    KIND_GLOBAL_ROLE, KIND_GLOBAL_ROLE_BINDING, KIND_ROLE, KIND_ROLE_BINDING,
    KIND_SERVICE_ACCOUNT, KIND_SERVICE_ACCOUNT_TOKEN,
};
use crate::rbac::helpers::object_string_id;
use crate::rbac::middleware::Claims;

// ── Private helpers ───────────────────────────────────────────────────────────

/// Resolve `kind` to its canonical name through the alias cache.
///
/// Returns [`DawnStoreError::UnknownResourceKind`] when the alias is not registered.
async fn resolve_kind(cache: &DawnstoreCache, kind: &str) -> Result<String, DawnStoreError> {
    cache
        .resolve_kind(kind)
        .await
        .ok_or_else(|| DawnStoreError::UnknownResourceKind(kind.to_string()))
}

/// Load the effective permissions for `caller` from the cache, rebuilding from
/// `backend` on a miss.
async fn get_or_load_permissions<B: DawnstoreBackend>(
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

/// Verify that `caller` holds `Delete` permission on `(namespace, kind, name)`.
///
/// When `caller` is `None` (unauthenticated / superadmin path) or the caller is
/// the system superadmin SA, the check is skipped and `Ok(())` is returned.
///
/// Namespace-scoped grants (from `RoleBinding`s) are only honoured when the
/// target `namespace` matches the caller's own namespace. Global grants apply
/// across all namespaces.
///
/// Returns [`DawnStoreError::Forbidden`] if the caller lacks the required permission.
async fn check_delete_permission<B: DawnstoreBackend>(
    cache: &DawnstoreCache,
    backend: &B,
    caller: Option<&Claims>,
    namespace: &str,
    kind: &str,
    name: &str,
) -> Result<(), DawnStoreError> {
    let Some(caller) = caller else {
        return Ok(()); // unauthenticated / superadmin path
    };
    if is_superadmin(caller) {
        return Ok(());
    }

    let perms = get_or_load_permissions(cache, backend, caller).await?;

    let verb_kind_name = |scope: &crate::cache::GrantedScope| {
        scope.verbs.contains(&Verb::Delete)
            && scope.kinds.iter().any(|k| k == "*" || k == kind)
            && scope.names.as_ref().map_or(true, |names| names.iter().any(|n| n == name))
    };

    // Namespace-scoped grants are only valid within the caller's own namespace.
    let allowed = (caller.namespace == namespace && perms.namespaced.iter().any(verb_kind_name))
        || perms.global.iter().any(verb_kind_name);

    if allowed {
        Ok(())
    } else {
        Err(DawnStoreError::Forbidden)
    }
}

/// Returns `true` if `kind` is an RBAC resource whose deletion must trigger
/// permission-cache invalidation.
fn is_rbac_kind(kind: &str) -> bool {
    matches!(kind, KIND_ROLE | KIND_GLOBAL_ROLE | KIND_ROLE_BINDING | KIND_GLOBAL_ROLE_BINDING)
}

/// Returns `true` if `kind` is a `ServiceAccountToken` whose deletion must
/// revoke the corresponding JWT from the token-validity cache.
fn is_token_kind(kind: &str) -> bool {
    kind == KIND_SERVICE_ACCOUNT_TOKEN
}

/// Returns `true` if `kind` is a `ServiceAccount` whose deletion must evict
/// its permission cache entry.
fn is_service_account_kind(kind: &str) -> bool {
    kind == KIND_SERVICE_ACCOUNT
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Delete the object identified by `request`, enforcing RBAC for authenticated callers.
///
/// Steps:
/// 1. Resolve `request.kind` through the kind-alias cache.
///    Returns [`DawnStoreError::UnknownResourceKind`] if the alias is not registered.
/// 2. If `caller` is authenticated (and is not the system superadmin), verify that
///    the caller holds `Delete` permission on `(namespace, kind, name)`.
///    Returns [`DawnStoreError::Forbidden`] if denied.
/// 3. Check that no other objects hold an inbound FK relation to this object.
///    Returns [`DawnStoreError::DeleteBlockedByReferences`] if any exist, because
///    deleting the object would leave those FK fields pointing at a non-existent target.
/// 4. Delete the object from the backend (idempotent — no error if it doesn't exist).
/// 5. If the deleted object is an RBAC resource (`Role`, `RoleBinding`, `GlobalRole`,
///    or `GlobalRoleBinding`), evict all permission-cache entries derived from it so
///    that downstream Get / Apply checks reflect the change immediately.
pub async fn delete<B: DawnstoreBackend>(
    backend: &B,
    cache: &DawnstoreCache,
    caller: Option<&Claims>,
    request: DeleteObject,
) -> Result<(), DawnStoreError> {
    // Step 1: resolve kind alias.
    let kind = resolve_kind(cache, &request.kind).await?;
    let namespace = request.namespace.as_deref().unwrap_or("default");

    // Step 2: permission check.
    check_delete_permission(cache, backend, caller, namespace, &kind, &request.name).await?;

    // Step 3: reject if other objects still reference this one via FK relations.
    let refs = backend.get_inbound_references(namespace, &kind, &request.name).await?;
    if !refs.is_empty() {
        return Err(DawnStoreError::DeleteBlockedByReferences {
            target: object_string_id(namespace, &kind, &request.name),
            referencing: refs.join(", "),
        });
    }

    // Step 4: if this is a ServiceAccountToken, fetch its UUID before deletion
    // so we can revoke the corresponding JWT from the token-validity cache.
    let token_id_to_revoke = if is_token_kind(&kind) {
        backend.get_object(namespace, &kind, &request.name).await?.map(|o| o.id)
    } else {
        None
    };

    // Step 5: delete from backend.
    backend.delete_object(namespace, &kind, &request.name).await?;

    // Step 6: invalidate RBAC cache for deleted RBAC resources.
    if is_rbac_kind(&kind) {
        cache.invalidate_permissions(&object_string_id(namespace, &kind, &request.name));
    }

    // Step 7: revoke the JWT for a deleted ServiceAccountToken.
    if let Some(token_id) = token_id_to_revoke {
        cache.remove_token(token_id);
    }

    // Step 8: evict the permission cache entry for a deleted ServiceAccount so
    // that re-creating an SA with the same (namespace, name) does not inherit
    // stale grants from the old identity.
    if is_service_account_kind(&kind) {
        cache.invalidate_sa_permissions(namespace, &request.name);
    }

    Ok(())
}
