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

- **`dawnstore-core`** — Core business logic. Feature-gated: `postgres` feature enables the `PostgresBackend`, `axum` feature enables the HTTP controller routes. Contains:
  - `backends/postgres/` — `PostgresBackend` struct with `apply_raw`, `get`, `delete`, `get_resource_definitions`, `get_object_infos`, `seed_object_schema`
  - `backends/postgres/apply_impl.rs` — Object parsing, JSON schema validation, foreign key checking, upsert logic
  - `backends/postgres/queries.rs` — All sqlx queries (compile-time checked)
  - `backends/postgres/data_models.rs` — Internal DB structs (`Object`, `ObjectSchema`, `ForeignKeyConstraint`, `Relation`)
  - `controllers.rs` — Axum route handlers that delegate to `PostgresBackend`
  - `models.rs` — `ForeignKey`, `ForeignKeyType`, `ForeignKeyBehaviour`, and example domain types (`TestCar`, `Container`)
  - `error.rs` — `DawnStoreError` enum

- **`dawnstore-api`** — Binary that wires up `PostgresBackend` + Axum. Reads `DATABASE_URL` env var, runs migrations, seeds schemas, serves on `:8080`.

- **`dawnstore-client-lib`** — HTTP client (`Api` struct) wrapping reqwest to call the dawnstore API. Re-exports `dawnstore-lib` types.

- **`dawnstore-cli`** — kubectl-like CLI (`get`, `delete`, `edit`, `apply`). Uses `DAWNSTORE_CONTEXT` env var or `-c` flag pointing to a YAML context file with a `url` field.

### Core Concepts

**Object model**: Every stored object has Kubernetes-like metadata (`namespace`, `kind`, `api_version`, `name`, `labels`, `annotations`) plus a typed `spec` stored as JSONB. Objects are identified by the composite string `{namespace}/{kind}/{name}`.

**Resource definitions**: Before storing objects of a given `kind`, the schema must be registered via `seed_object_schema::<T>()`, which derives a JSON schema from a Rust type using `schemars`. This schema is validated on every `apply`.

**Foreign keys**: Relations between objects are declared at schema registration time using `ForeignKey` structs (path, type, optional kind constraint). Types are `One`, `OneOptional`, `OneOrMany`, `NoneOrMany`. FK values in object specs are strings formatted as `name`, `kind/name`, or `namespace/kind/name`. Relations are stored in a separate `relations` table and can be populated into responses via `fill_child_foreign_keys`.

**Apply semantics**: `POST /apply` accepts a single object, an array, or a `List` wrapper object. It upserts objects (insert-or-update by UUID, keyed on string_id) and reconciles the relations table.

**Migrations**: Located at `dawnstore-core/migrations/`. Run via sqlx's `sqlx::migrate!("./migrations")` macro — path is relative to the `dawnstore-core` crate root.

### API Endpoints

| Method | Path | Body |
|--------|------|------|
| `POST` | `/apply` | `Object`, `[Object]`, or `ListOfObjects` JSON |
| `POST` | `/get-objects` | `GetObjectsFilter` |
| `POST` | `/get-object-infos` | `GetObjectInfosFilter` |
| `POST` | `/get-resource-definitions` | `GetResourceDefinitionFilter` |
| `DELETE` | `/delete-object` | `DeleteObject` |

### In-memory caches

`PostgresBackend` holds two `RwLock<HashMap>` caches populated lazily on first access:
- `schema_cache`: `{api_version}/{kind}` → compiled `jsonschema::Validator`
- `foreign_key_cache`: `{api_version}/{kind}` → `Vec<ForeignKeyConstraint>`
