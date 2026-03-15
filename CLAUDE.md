# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```bash
# Build all crates
cargo build

# Build a specific crate
cargo build -p dawnstore-api

# Run tests
cargo test

# Run tests for a specific crate
cargo test -p dawnstore-core

# Run a single test by name
cargo test -p dawnstore-core test_name

# Run the API server (requires DATABASE_URL env var)
DATABASE_URL=postgres://... cargo run -p dawnstore-api

# Run the CLI
DAWNSTORE_CONTEXT=./context.yml cargo run -p dawnstore-cli -- get all

# Run sqlx migrations (done automatically on server startup via backend.sqlx_migrate())
```

## Architecture

This is a Cargo workspace with 5 crates implementing a Kubernetes-like generic object store backed by PostgreSQL.

### Crate Overview

- **`dawnstore-lib`** — Shared types used across all crates: `Object<T>`, `ReturnObject<T>`, `GetObjectsFilter`, `DeleteObject`, `ResourceDefinition`, `ObjectInfo`, etc. No dependencies on other workspace crates.

- **`dawnstore-core`** — Core business logic. Contains:
  - `abstractions.rs` — `DawnstoreBackend` trait (existing), `NewDawnStoreBackend` trait (refactor placeholder), `ResourceCache`, `SchemaDefinition`, `ForeignKey`/`ForeignKeyType`/`ForeignKeyBehaviour`, `RawSchema`, `RawForeignKeyConstraint`, `BackendGetObjectsFilter`
  - `cache.rs` — `DawnstoreCache`: unified tokio-RwLock-backed cache for schema validators (`api_version/kind` → `Arc<jsonschema::Validator>`), FK constraints (`api_version/kind` → `Arc<Vec<RawForeignKeyConstraint>>`), and RBAC permissions. Populated via `init`, `init_schema`, `init_foreign_key`, `init_permission` (each takes a `&impl NewDawnStoreBackend`). Cache keys are built with `schema_cache_key` from `rbac/helpers.rs`.
  - `controllers.rs` — Axum route handlers that delegate to `DawnstoreBackend`
  - `error.rs` — `DawnStoreError` enum
  - `rbac/` — RBAC subsystem: models, JWT service, middleware, authz service, permission cache (`RbacCache`), helpers (`object_string_id`, `schema_cache_key`)

- **`dawnstore-api`** — Binary that wires up `PostgresBackend` + Axum. Reads `DATABASE_URL` env var, runs migrations, seeds schemas, serves on `:8080`.

- **`dawnstore-client-lib`** — HTTP client (`Api` struct) wrapping reqwest to call the dawnstore API. Re-exports `dawnstore-lib` types.

- **`dawnstore-cli`** — kubectl-like CLI (`get`, `delete`, `edit`, `apply`). Uses `DAWNSTORE_CONTEXT` env var or `-c` flag pointing to a YAML context file with a `url` field.

### Core Concepts

**Object model**: Every stored object has Kubernetes-like metadata (`namespace`, `kind`, `api_version`, `name`, `labels`, `annotations`) plus a typed `spec` stored as JSONB. Objects are identified by the composite string `{namespace}/{kind}/{name}`.

**Resource definitions**: Before storing objects of a given `kind`, the schema must be registered via `seed_object_schema::<T>()`, which derives a JSON schema from a Rust type using `schemars`. This schema is validated on every `apply`.

**Foreign keys**: Relations between objects are declared at schema registration time using `ForeignKey` structs (path, type, optional kind constraint). Types are `One`, `OneOptional`, `OneOrMany`, `NoneOrMany`. FK values in object specs are strings formatted as `name`, `kind/name`, or `namespace/kind/name`. Relations are stored in a separate `relations` table and can be populated into responses via `fill_child_foreign_keys`.

**Apply semantics**: `POST /apply` accepts a single object, an array, or a `ListOfObjects` wrapper. It upserts objects (insert-or-update by UUID, keyed on string_id) and reconciles the relations table.

`ListOfObjects` supports **implied properties**: if `object_kind` and/or `object_api_version` are set on the list wrapper, any item inside `list` that is missing its own `kind` or `api_version` inherits those values. This means callers can omit `kind`/`api_version` on every individual item when all objects in the list share the same kind and version.

**Migrations**: Located at `dawnstore-core/migrations/`. Run via sqlx's `sqlx::migrate!("./migrations")` macro — path is relative to the `dawnstore-core` crate root.

### API Endpoints

| Method | Path | Body |
|--------|------|------|
| `POST` | `/apply` | `Object`, `[Object]`, or `ListOfObjects` JSON |
| `POST` | `/get-objects` | `GetObjectsFilter` |
| `POST` | `/get-object-infos` | `GetObjectInfosFilter` |
| `POST` | `/get-resource-definitions` | `GetResourceDefinitionFilter` |
| `DELETE` | `/delete-object` | `DeleteObject` |

### Shared helpers and constants

**Always use the existing helpers and constants from `rbac/helpers.rs` and `rbac/constants.rs` when writing new code — never inline equivalent strings or logic.**

- `rbac/helpers.rs` — formatting helpers:
  - `object_string_id(namespace, kind, name)` — canonical `namespace/kind/name` string ID for any dawnstore object
  - `schema_cache_key(api_version, kind)` — cache key in the form `api_version/kind` used by `DawnstoreCache`
- `rbac/constants.rs` — all well-known kind names (`KIND_NAMESPACE`, `KIND_ROLE`, `KIND_ROLE_BINDING`, `KIND_GLOBAL_ROLE`, `KIND_GLOBAL_ROLE_BINDING`, `KIND_SERVICE_ACCOUNT`, `KIND_SERVICE_ACCOUNT_TOKEN`), the `SYSTEM_NAMESPACE` name, and other fixed string values

### In-memory caches

**`dawnstore-core` (`DawnstoreCache`)** — the canonical, backend-agnostic cache. One struct with three tokio `RwLock`-backed stores:
- `schema`: `{api_version}/{kind}` → `Arc<jsonschema::Validator>`
- `foreign_key`: `{api_version}/{kind}` → `Arc<Vec<RawForeignKeyConstraint>>`
- `permission`: `(namespace, sa_name)` → `EffectivePermissions` (with a resource index for targeted invalidation)

Initialised via `DawnstoreCache::init(backend)` or per-store via `init_schema`, `init_foreign_key`, `init_permission`. All init functions take `&impl NewDawnStoreBackend`.

**`dawnstore-postgres` (`CacheStore`)** — legacy postgres-specific cache, kept during the refactor. Will be superseded by `DawnstoreCache`.

### Navigation Properties

Nav-props are spec fields whose name ends in `_object` (singular FK) or `_objects` (list FK). They are **not stored** — they are populated on GET when `fill_child_foreign_keys: true` is set, and stripped before storage on apply.

**On apply** (`handlers/apply.rs::extract_navigation_properties`): fields ending in `_object`/`_objects` are removed from the parent spec and prepended to the batch as independent objects. They go through the full apply pipeline (permission check, schema validation, FK walk, upsert). Nav-prop extraction runs **before** schema validation, so schemas with `deny_unknown_fields` never see those fields.

**On GET** (`dawnstore-postgres`): FK target objects are fetched and injected under the corresponding field name. The injected objects are filtered against `filter.allowed` (the caller's effective permission scopes) before being returned, so callers cannot read FK targets they lack Get permission on.

**Naming convention for model fields:**
- `One` / `OneOptional` FK → `<field>_object: Option<ReturnObject<Box<T>>>`
- `OneOrMany` / `NoneOrMany` FK → `<field>_objects: Option<Vec<ReturnObject<Box<T>>>>`

### RBAC Permission Model

**Namespace scoping is asymmetric across verbs:**
- **GET**: namespace-scoped grants (from `RoleBinding`) are correctly restricted to the caller's own namespace via `AllowedScope` passed to the backend.
- **Apply / Delete**: the `_namespace` parameter in `check_permission` and `check_delete_permission` is currently **unused** — namespace-scoped grants effectively apply across all namespaces for write operations. This is a known bug (tracked in `docs/todo.md`).

**Cache invalidation** triggers on apply/delete of `role`, `globalrole`, `rolebinding`, `globalrolebinding`. It does **not** trigger on `serviceaccount` deletion (known gap).

**Superadmin** is identified by `claims.namespace == "system" && claims.sub == "superadmin"` — no DB lookup. `is_superadmin` lives in `cache.rs` and bypasses all permission checks.

**Token issuance**: `/rbac/issue-token` is superadmin-only. It calls `backend.upsert_objects` directly (bypassing the normal apply flow) but explicitly verifies the referenced `ServiceAccount` exists before creating the token.

### Known Security Issues (open, tracked in docs/todo.md)

| # | Issue | Location |
|---|-------|----------|
| 1 | JWT not revocable — deleting token object has no effect | `jwt_service.rs`, `middleware.rs` |
| 2 | Apply/Delete namespace check uses `_namespace` (unused) — namespace-scoped grants apply cross-namespace | `handlers/apply.rs`, `handlers/delete.rs` |
| 3 | Namespace restriction bypassed by embedding a `Namespace` object in a nav-prop field | `controllers.rs`, `handlers/apply.rs` |
| 4 | No RBAC escalation prevention — Apply on `role`/`rolebinding` allows self-elevation | `handlers/apply.rs` |
| 5 | ~~`issue_token` skips FK validation — token can reference a non-existent `ServiceAccount`~~ **FIXED** | `controllers.rs` |
| 6 | ~~Deleting a `ServiceAccount` does not invalidate its cached permissions~~ **FIXED** | `handlers/delete.rs`, `cache.rs` |
| 7 | ~~Concurrent cache miss → N parallel full DB scans with no deduplication~~ **FIXED** | `cache.rs` |
| 8 | Object names with `/` create ambiguous string IDs | `handlers/apply.rs`, `rbac/helpers.rs` |
| 9 | Any `_object`/`_objects` spec field is silently extracted; failed deserialization is swallowed | `handlers/apply.rs` |
| 10 | No request body size limit | All handlers |
