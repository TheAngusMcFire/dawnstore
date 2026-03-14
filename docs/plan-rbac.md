# RBAC Implementation Plan

## Overview

The RBAC enforcement layer lives entirely in `dawnstore-core`. It consists of three
parts: an in-memory permission cache, an authorization checker that uses it, and
Axum middleware that integrates the checker into the request path.

---

## 1. RbacCache — in-memory permission store (`dawnstore-core/src/rbac/cache.rs`)

### What it stores

The cache maps a **service account identity** to its **effective permission set**:

```
RbacCache:
  HashMap<(namespace: String, sa_name: String), EffectivePermissions>
```

`EffectivePermissions` is the collapsed union of all `role`/`globalrole` rules bound
to that service account at the time of the last cache fill:

```rust
struct EffectivePermissions {
    // Namespace-scoped grants: (namespace, kind) → allowed names (None = all)
    namespaced: Vec<GrantedScope>,
    // Global grants that apply across all namespaces
    global: Vec<GrantedScope>,
}

struct GrantedScope {
    api_version: String,   // "*" matches all
    kind: String,          // "*" matches all
    verbs: EnumSet<Verb>,  // get | apply | delete
    names: Option<Vec<String>>, // None = all names
}
```

### Cache invalidation tracking

Every cache entry is tagged with the set of dawnstore **object names** that
contributed to it:

```
resource_index: HashMap<String, HashSet<(namespace, sa_name)>>
  key:   "{namespace}/{kind}/{name}"  e.g. "myns/role/editor"
  value: all (namespace, sa) cache keys whose permissions were derived from this object
```

When `apply` or `delete` touches a role / role-binding / global-role / global-role-binding
object the affected SA entries are looked up in `resource_index` and evicted from the
main cache. The next access for those SAs triggers a DB fallback and re-population.

### `warm(&backend)`

Called at startup (after `rbac::init`). Loads all `rolebinding`, `globalrolebinding`,
`role`, and `globalrole` objects from the backend, computes `EffectivePermissions` for
every referenced service account, and populates both `HashMap`s.

### Cache miss / DB fallback

On a cache miss for `(namespace, sa)`:
1. Query the DB for all `rolebinding`s in `namespace` whose `subjects` include the SA.
2. For each binding, fetch the linked `role`.
3. Query all `globalrolebinding`s whose `subjects` include the SA.
4. For each binding, fetch the linked `globalrole`.
5. Collapse rules → `EffectivePermissions`, insert into cache, update `resource_index`.

This means the DB is only hit when the cache is cold or has been explicitly invalidated.
The cache is **not** time-based; entries live until the underlying RBAC object changes.

---

## 2. Authorization checker (`dawnstore-core/src/rbac/authz_service.rs`)

Add to the existing `authz_service` module:

### `is_allowed(cache, backend, claims, verb, namespace, kind, name) -> Result<bool>`

1. `is_superadmin(claims)` → immediately return `true`.
2. Look up `(claims.namespace, claims.sub)` in cache.
   - Cache miss → run DB fallback, populate, retry.
3. Walk `EffectivePermissions`:
   - Match any `GrantedScope` where `verb` is in `verbs`, `api_version`/`kind` match
     (respecting wildcards), and `name` matches `names` constraint.
4. Return `true` if any scope matches, `false` otherwise.

### `allowed_scopes(cache, backend, claims, verb) -> Result<Option<Vec<AllowedScope>>>`

Used for `get` queries to build the SQL filter constraint (see architecture doc):

1. `is_superadmin(claims)` → return `None` (unrestricted).
2. Collect all matching `GrantedScope` entries for the given verb.
3. Expand into `Vec<AllowedScope>` (see `GetObjectsFilter` extension below).
4. Empty list → deny immediately (caller returns `[]`, no DB hit).

---

## 3. Foreign key extraction split

Currently `apply_impl.rs` in the postgres backend bundles:
- **extraction** — walking the object spec to find FK string values
- **resolution** — turning those strings into DB object IDs
- **validation** — checking the referenced objects exist

These need to be split so the authorization layer can inspect FK targets
before the backend touches the DB:

### `fk_extractor.rs` (new, in `dawnstore-core`)

```rust
/// Extract raw FK string values from an object spec given the FK constraints.
/// Returns Vec<(constraint_id, Vec<string_id>)> — no DB access.
fn extract_fk_values(
    obj: &ObjectAny,
    constraints: &[ForeignKeyConstraint],
) -> Result<Vec<(Uuid, Vec<String>)>, DawnStoreError>
```

This replaces the extraction portion currently inlined in `apply_impl::check_foreign_keys`.

### FK access check in middleware

After extraction but before the backend writes anything:

```rust
for (_, string_ids) in &fk_values {
    for sid in string_ids {
        let (ns, kind, name) = parse_string_id(sid);
        if !is_allowed(cache, backend, claims, Verb::Get, ns, kind, name).await? {
            return Err(Forbidden);
        }
    }
}
```

The backend's `check_foreign_keys` retains the DB-existence validation;
only access checking moves to the middleware.

---

## 4. `GetObjectsFilter` extension

Add to `dawnstore-lib`:

```rust
pub struct AllowedScope {
    pub namespace: String,
    pub kind: String,
    pub names: Option<Vec<String>>,
}

// In GetObjectsFilter:
pub allowed: Option<Vec<AllowedScope>>,
// None        = unrestricted (superadmin)
// Some([...]) = restrict results to these (namespace, kind[, names])
```

The postgres backend translates this into a SQL constraint in `get_objects_by_filter`.

---

## 5. Permission enforcement in controllers (`dawnstore-core/src/controllers.rs`)

Permission checks are called directly inside the existing controller handlers —
no additional Axum middleware layer is introduced. The JWT middleware continues to
handle authentication only (token validation, claims extraction into request extensions).

### `apply` handler
1. Extract `Claims` from the request extension (set by the JWT middleware).
2. Pre-parse `(namespace, kind, name)` from the JSON body.
3. Call `is_allowed(cache, backend, claims, Verb::Apply, namespace, kind, name)`.
4. Extract FK values using `fk_extractor` and call `is_allowed(..., Verb::Get, ...)` on each target.
5. Return `403` in the `DawnStoreResponse` envelope if any check fails; otherwise proceed.

### `get-objects` / `get-object-infos` handlers
1. Extract `Claims` from the request extension.
2. Call `allowed_scopes(cache, backend, claims, Verb::Get)`.
3. Inject the resulting `allowed` field into `GetObjectsFilter` / `GetObjectInfosFilter`.
4. Forward to the backend (returns `[]` immediately if `allowed` is `Some([])`).

### `delete-object` handler
1. Extract `Claims` from the request extension.
2. Call `is_allowed(cache, backend, claims, Verb::Delete, namespace, kind, name)`.
3. Return `403` in the envelope if denied; otherwise proceed.

### Cache invalidation on mutating requests
After a successful `apply` or `delete` on an RBAC resource kind
(`role`, `globalrole`, `rolebinding`, `globalrolebinding`):
invalidate affected cache entries via the `resource_index`.

---

## 6. Implementation order

1. `fk_extractor.rs` — split extraction from `apply_impl`, add unit tests.
2. `AllowedScope` + `allowed` field in `GetObjectsFilter` / postgres query.
3. `RbacCache` struct with `warm`, DB fallback, and `resource_index`.
4. `authz_service` additions: `is_allowed`, `allowed_scopes`.
5. Middleware enforcement: `apply`, `get`, `delete`, `get-object-infos`.
6. Cache invalidation hook in middleware post-apply/delete.
7. End-to-end tests covering: allow, deny, cache invalidation, FK access check.
