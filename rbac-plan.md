# RBAC Plan

## Overview

A Kubernetes-inspired RBAC system built on top of the dawnstore object model.
Authentication uses JWTs derived from `ServiceAccountToken` objects.
Authorization uses `Role`/`GlobalRole` bound to `ServiceAccount` via `RoleBinding`/`GlobalRoleBinding`.
All models are stored as dawnstore objects, making the system backend-agnostic.

---

## Verbs

| Verb     | Description                   |
|----------|-------------------------------|
| `get`    | Read objects                  |
| `apply`  | Create or update objects      |
| `delete` | Delete objects                |

---

## Models

### ServiceAccount
Represents an identity (a service or user).

| Field       | Type     | Notes                          |
|-------------|----------|--------------------------------|
| `name`      | `String` | Unique within namespace        |
| `namespace` | `String` | Inherited from dawnstore model |

A built-in superadmin `ServiceAccount` (e.g. `namespace: system`, `name: superadmin`) bypasses all authorization checks.

---

### ServiceAccountToken
A named credential bound to a `ServiceAccount`. Generating a JWT requires this object to exist and not be expired.

| Field             | Type              | Notes                              |
|-------------------|-------------------|------------------------------------|
| `name`            | `String`          |                                    |
| `service_account` | `String`          | FK → `ServiceAccount`              |
| `expires_at`      | `Option<DateTime>`| `None` = never expires             |

**JWT claims**: `sub` (service_account name), `namespace`, `token_name`, `token_id` (UUID of the `ServiceAccountToken` object), `exp`.
The JWT is signed with a server secret; no token material is stored in the DB.

---

### Role
A namespace-scoped set of permissions.

| Field       | Type              | Notes                   |
|-------------|-------------------|-------------------------|
| `name`      | `String`          |                         |
| `namespace` | `String`          |                         |
| `rules`     | `Vec<PolicyRule>` |                         |

#### PolicyRule

| Field        | Type                  | Notes                                        |
|--------------|-----------------------|----------------------------------------------|
| `api_version`| `String`              | `"*"` matches all                            |
| `kinds`      | `Vec<String>`         | `["*"]` matches all                          |
| `verbs`      | `Vec<String>`         | subset of `get`, `apply`, `delete`           |
| `names`      | `Option<Vec<String>>` | `None` = all objects; `Some([...])` = restrict to specific object names |

---

### GlobalRole
Cluster-scoped role. Rules apply across all namespaces.

| Field   | Type              | Notes |
|---------|-------------------|-------|
| `name`  | `String`          |       |
| `rules` | `Vec<PolicyRule>` |       |

---

### RoleBinding
Binds a `Role` to one or more `ServiceAccount`s within a namespace.

| Field      | Type          | Notes                              |
|------------|---------------|------------------------------------|
| `name`     | `String`      |                                    |
| `namespace`| `String`      | Must match the referenced Role     |
| `role`     | `String`      | FK → `Role` (same namespace)       |
| `subjects` | `Vec<Subject>`|                                    |

#### Subject

| Field       | Type     | Notes                      |
|-------------|----------|----------------------------|
| `name`      | `String` | ServiceAccount name        |
| `namespace` | `String` | ServiceAccount namespace   |

---

### GlobalRoleBinding
Binds a `GlobalRole` to one or more `ServiceAccount`s across all namespaces.

| Field       | Type          | Notes                   |
|-------------|---------------|-------------------------|
| `name`      | `String`      |                         |
| `role`      | `String`      | FK → `GlobalRole`       |
| `subjects`  | `Vec<Subject>`|                         |

---

## Authentication Flow

1. Client presents a JWT in the `Authorization: Bearer <token>` header.
2. Server validates the JWT signature and `exp` claim.
3. Server looks up the `ServiceAccountToken` by `token_id` (UUID) from the JWT claims and verifies it still exists, belongs to the claimed `ServiceAccount`, and is not expired.
4. The resolved `ServiceAccount` becomes the request identity.

---

## Authorization Flow

1. If the `ServiceAccount` is the superadmin → **allow**.
2. Collect all `RoleBinding`s in the target namespace that list the `ServiceAccount` as a subject.
3. Check if any bound `Role`'s rules permit the requested `(api_version, kind, verb)`.
4. Collect all `GlobalRoleBinding`s that list the `ServiceAccount` as a subject.
5. Check if any bound `GlobalRole`'s rules permit the requested `(api_version, kind, verb)`.
6. If no rule matches → **deny**.

---

## Efficient Query Authorization

Since permissions are granted at the kind level (not per-object), the middleware resolves the full set of permitted `(namespace, kind, names)` tuples before the query reaches the backend and injects them as a filter constraint.

### GetObjectsFilter extension

```
allowed: Option<Vec<AllowedScope>>
// None        = unrestricted (superadmin)
// Some([...]) = only return objects matching one of the scopes
```

```
AllowedScope {
    namespace: String,
    kind:      String,
    names:     Option<Vec<String>>,  // None = all names in this (namespace, kind)
}
```

### Flow for `get` requests

1. Middleware resolves all `Role`/`GlobalRole` rules with verb `get` for the SA.
2. Collapses them into a list of `AllowedScope` entries.
   - A GlobalRole with `kinds: ["*"]` and `names: None` → `allowed = None` (full access).
   - If the resulting list is empty → return `[]` immediately, no DB hit.
3. Injects `allowed` into `GetObjectsFilter` and forwards to the backend.
4. Backend translates scopes into a SQL constraint (single query, no post-filtering):
   ```sql
   WHERE (namespace, kind) = ANY($scopes)
     AND (name = ANY($names) OR $names IS NULL)
   ```

### Flow for `apply` / `delete` (single-object operations)

Simple boolean check: does the SA have the verb for `(api_version, kind, namespace, name)`? Deny with `403` before touching the backend.

### Trait expansion

`GetObjectsFilter` gains the `allowed` field. No new trait methods are required.

---

## Middleware Integration

RBAC will be implemented as an Axum middleware layer wrapping the existing routes.
The middleware extracts and validates the JWT, resolves permissions, and rejects unauthorized requests with `401`/`403` before the request reaches the backend.
