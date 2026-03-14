# RBAC Architecture

## Overview

A Kubernetes-inspired RBAC system built on top of the dawnstore object model.
Authentication uses JWTs derived from `serviceaccounttoken` objects.
Authorization uses `role`/`globalrole` bound to `serviceaccount` via `rolebinding`/`globalrolebinding`.
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

### serviceaccount (aliases: `serviceaccounts`, `sa`)
Represents an identity (a service or user). The spec is empty; identity is fully expressed by `name` and `namespace` from the dawnstore object metadata.

A built-in superadmin `serviceaccount` (`namespace: system`, `name: superadmin`) bypasses all authorization checks.

---

### serviceaccounttoken (aliases: `serviceaccounttokens`, `sat`)
A named credential bound to a `serviceaccount`. Generating a JWT requires this object to exist and not be expired.

| Field             | Type                          | Notes                                        |
|-------------------|-------------------------------|----------------------------------------------|
| `service_account` | `String`                      | FK → `serviceaccount` (same namespace)       |
| `expires_at`      | `Option<DateTime<Utc>>`       | `None` = never expires                       |

**JWT claims**: `sub` (serviceaccount name), `namespace`, `token_name`, `token_id` (UUID of the `serviceaccounttoken` object), `exp`.
The JWT is signed with a server secret; no token material is stored in the DB.

---

### role (aliases: `roles`, `ro`)
A namespace-scoped set of permissions.

| Field   | Type              | Notes |
|---------|-------------------|-------|
| `rules` | `Vec<PolicyRule>` |       |

#### PolicyRule

| Field        | Type                  | Notes                                                                   |
|--------------|-----------------------|-------------------------------------------------------------------------|
| `api_version`| `String`              | `"*"` matches all                                                       |
| `kinds`      | `Vec<String>`         | `["*"]` matches all                                                     |
| `verbs`      | `Vec<String>`         | subset of `get`, `apply`, `delete`                                      |
| `names`      | `Option<Vec<String>>` | `None` = all objects; `Some([...])` = restrict to specific object names |

---

### globalrole (aliases: `globalroles`, `gr`)
Cluster-scoped role. Rules apply across all namespaces.

| Field   | Type              | Notes |
|---------|-------------------|-------|
| `rules` | `Vec<PolicyRule>` |       |

---

### rolebinding (aliases: `rolebindings`, `rb`)
Binds a `role` to one or more `serviceaccount`s within a namespace.

| Field      | Type          | Notes                                                        |
|------------|---------------|--------------------------------------------------------------|
| `role`     | `String`      | FK → `role` (same namespace)                                 |
| `subjects` | `Vec<String>` | FK → `serviceaccount`; each value is `namespace/serviceaccount/name` |

---

### globalrolebinding (aliases: `globalrolebindings`, `grb`)
Binds a `globalrole` to one or more `serviceaccount`s across all namespaces.

| Field      | Type          | Notes                                                        |
|------------|---------------|--------------------------------------------------------------|
| `role`     | `String`      | FK → `globalrole`                                            |
| `subjects` | `Vec<String>` | FK → `serviceaccount`; each value is `namespace/serviceaccount/name` |

---

## Authentication Flow

1. Client presents a JWT in the `Authorization: Bearer <token>` header.
2. Server validates the JWT signature and `exp` claim.
3. Server looks up the `serviceaccounttoken` by `token_id` (UUID) from the JWT claims and verifies it still exists, belongs to the claimed `serviceaccount`, and is not expired.
4. The resolved `serviceaccount` becomes the request identity.

---

## Authorization Flow

1. If the `serviceaccount` is the superadmin → **allow**.
2. Collect all `rolebinding`s in the target namespace that list the `serviceaccount` as a subject.
3. Check if any bound `role`'s rules permit the requested `(api_version, kind, verb)`.
4. Collect all `globalrolebinding`s that list the `serviceaccount` as a subject.
5. Check if any bound `globalrole`'s rules permit the requested `(api_version, kind, verb)`.
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

1. Middleware resolves all `role`/`globalrole` rules with verb `get` for the SA.
2. Collapses them into a list of `AllowedScope` entries.
   - A `globalrole` with `kinds: ["*"]` and `names: None` → `allowed = None` (full access).
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
