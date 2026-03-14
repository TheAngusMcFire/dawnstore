# Cleanup TODO: Remove Old Implementation

Now that all controllers and tests use `get_dawnstore_new_routes` + `DawnstoreCache`,
the old implementation can be removed in two phases.

## Phase 1 — Remove immediately (no blockers)

These are in `dawnstore-core/src/controllers.rs`:

- `get_dawnstore_default_routes` function and its `ApiState<B>` state struct
- Old handler functions: `apply`, `get_objects`, `get_object_infos`, `get_resource_definitions`, `delete_object`
- `check_namespace_restriction` (sync version — replaced by `check_namespace_restriction_cached`)
- `extract_apply_identities`
- `is_rbac_kind` (local helper in old controller — the one in `handlers/apply.rs` is still used)
- Any remaining `RbacCache` imports in `controllers.rs`

## Phase 2 — Remove after RBAC refactor

The following are still used by `rbac::init`, `rbac::bootstrap`, `get_rbac_routes`,
`authz_service`, and `token_controller`. They can be removed once RBAC is ported to
`NewDawnStoreBackend`.

### `DawnstoreBackend` trait (`dawnstore-core/src/abstractions.rs`)
- The entire `DawnstoreBackend` trait and its associated types
- `impl DawnstoreBackend for PostgresBackend` in `dawnstore-postgres/src/lib.rs`

### `ResourceCache` struct (`dawnstore-core/src/abstractions.rs` or wherever it lives)
- `ResourceCache` and its `warm()` / `resolve_kind()` methods
- `CacheStore` (if separate)
- `warm_caches()` method on `PostgresBackend`

### `apply_impl.rs` (`dawnstore-core/src/backends/postgres/apply_impl.rs` or similar)
- Old apply implementation used only by `DawnstoreBackend::apply`

### In `dawnstore-api/src/main.rs`
- `backend.warm_caches().await?`
- `RbacCache::new()` + `rbac_cache.warm(...)` (once RBAC uses `DawnstoreCache`)

## Renames

- `NewDawnStoreBackend` → `DawnstoreBackend` (drop "New" prefix once old trait is gone)
- `get_dawnstore_new_routes` → `get_dawnstore_routes`
- `new_apply`, `new_get_objects`, `new_delete_object`, `new_get_resource_definitions` → drop `new_` prefix
- `NewApiState` → `ApiState`
