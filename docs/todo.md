* add fallback to the controllers in case no claims are injected in the controller
* issue token check if there is actually a serviceaccount in the namespace where we want to issue the token
* check for token duplicates for the create token cli command
* rework the script to store the keys and initial tokens in a file, do not reset the db all the time
* extract_apply_identities does not work for implied object infos
* the rbac cache should be decoupled from the backend
* global roles and global role bindings can only be created in the system namespace
* add validation for the role verbs or use enum for the role verbs

refactoring base functionality:
  * move the fk and schema cache to the core
  * create new handlers for apply_handler
  * we need to refactor the object preparation and validation to:
    * object preparation (unwrap lists)
    * check permissions of outer list of objects
    * check serialization
    * check schema of outer objects
    * check foreign keys and extract foreign key objects from the navigation properties
      * recursive
      * check permission of the objects extracted with the foreign keys
      * check schema of the nav props
      * check foreign keys 


checkout @dawnstore-api/src/models.rs there is a Container model, it contains the field parent_object this is a navigation property, a
  placeholder which is not written into the database, it gets filled in the get endpoint if the and

get_rbac_token_routes merge to normal routes

 ---
  1. JWT Token Not Revocable

  File: jwt_service.rs:85, middleware.rs:34

  validate_token only checks the signature and the exp field. The token_id UUID embedded in the claims is never verified against the
  database. Deleting a ServiceAccountToken object has no effect on any JWT already issued from it — those JWTs remain valid until they
  expire naturally.

  A compromised token cannot be revoked short of rotating the signing keypair (which invalidates all tokens system-wide).

  Test needed: Issue a token, delete the ServiceAccountToken object, verify the JWT is now rejected.

  ---
  2. Namespace-Scoped Apply/Delete Permissions Apply Across All Namespaces

  File: handlers/apply.rs:91, handlers/delete.rs:44

  Both check_permission (apply) and check_delete_permission (delete) take a namespace argument, but it is ignored (_namespace). The
  has_permission helper only inspects verb, kind, and name:

  fn has_permission(perms: &EffectivePermissions, verb: Verb, kind: &str, name: &str) -> bool {

  Namespace-scoped grants (from RoleBindings) are stored in perms.namespaced without carrying the namespace they were issued in. So a
  caller with a RoleBinding granting Apply on container in alice-ns can apply containers in any namespace.

  GET correctly enforces namespace scope (via AllowedScope passed to the backend), making the behaviour inconsistent across verbs.

  Test needed: Alice has a RoleBinding in alice-ns granting apply on container; she should be forbidden from applying a container in
  bob-ns.

  ---
  3. Namespace Restriction Bypass via Navigation Property Embedding

  File: controllers.rs:148 (check_namespace_restriction_cached), handlers/apply.rs:325 (extract_navigation_properties)

  The restriction that Namespace objects may only be created in the system namespace is enforced in apply_handler on the raw HTTP body
  before nav-prop extraction. Nav-props are stripped inside apply::apply(), which runs after the check.

  An attacker can embed a Namespace object in any field named *_object on an outer object:

  {
    "kind": "container", "namespace": "demo", "name": "x",
    "evil_object": { "kind": "namespace", "namespace": "demo", "name": "injected" }
  }

  The restriction check sees only kind=container and passes. The embedded namespace is then extracted, permission-checked (Apply on
  namespace), and upserted — outside the system namespace.

  Test needed: Submit a nav-prop-embedded namespace object targeting a non-system namespace; verify it is rejected.

  ---
  4. RBAC Privilege Escalation — No Escalation Prevention

  File: handlers/apply.rs:515 (apply)

  There is no check that a caller can only grant permissions they themselves hold. Any caller with Apply permission on role, globalrole,
  rolebinding, or globalrolebinding can:

  - Create a GlobalRole with verbs: ["get", "apply", "delete"], kinds: ["*"]
  - Bind it to their own ServiceAccount

  This is effectively a path from any write access on RBAC objects to full superadmin-equivalent permissions.

  Test needed: Alice has Apply on globalrole and globalrolebinding but nothing else; verify she cannot apply a global role that grants her
   more than she already has.

  ---
  5. issue_token Bypasses FK Validation — Token for Non-Existent Service Account

  File: controllers.rs:316

  The /rbac/issue-token endpoint builds a ServiceAccountToken and calls backend.upsert_objects(vec![obj_any], vec![]) directly, skipping
  the normal apply flow. The ServiceAccountToken.service_account field is declared as a FK to ServiceAccount, but that FK is never
  validated here.

  A superadmin can issue a valid, signed JWT for a service account name that does not exist. If a service account with that name is later
  created, the pre-issued JWT will inherit its permissions from that point forward — a pre-staging attack.

  Test needed: Superadmin issues a token for demo/sa/ghost (which does not exist); assert the call either errors or the resulting JWT has
  no permissions; then create the SA and verify the token cannot retroactively gain elevated permissions.

  ---
  6. Deleting a ServiceAccount Does Not Invalidate Its Cached Permissions

  File: handlers/delete.rs:79 (is_rbac_kind)

  is_rbac_kind returns true only for role, globalrole, rolebinding, globalrolebinding. Deleting a serviceaccount does not trigger
  cache.invalidate_permissions. Combined with the non-revocable JWT problem (#1), a deleted service account's JWT continues to work with
  full cached permissions until either the cache is rebuilt or those entries are naturally evicted.

  Test needed: Issue a token for SA Alice, delete the serviceaccount object, make a request with Alice's JWT, and verify it is rejected.

  ---
  7. Permission Cache Miss Triggers Uncapped Concurrent DB Scans

  File: cache.rs:174 (init_permission)

  On a cache miss, every concurrent request calls cache.init_permission(backend) independently — there is no lock or deduplication. Under
  load, N simultaneous requests for an uncached SA will trigger N full scans of all role, globalrole, rolebinding, and globalrolebinding
  objects.

  Test needed: Simulate concurrent requests from a new SA (cold cache) and verify the backend is not called more than once for the
  rebuild.

  ---
  8. Object Names with / Cause Ambiguous String IDs

  File: handlers/apply.rs:171 (resolve_fk_string), rbac/helpers.rs (object_string_id)

  String IDs are formed as namespace/kind/name. The FK parser uses split('/') (unbounded) to split these. If an object name contains /,
  the resulting string ID has more than 3 segments, causing FK references to that object to always fail with a validation error — but the
  object itself is stored successfully, creating an unreachable orphan.

  More subtly, object_string_id("ns", "kind", "a/b") = "ns/kind/a/b". Uses of splitn(3, '/') handle this correctly, but uses of split('/')
   do not, creating inconsistency.

  Test needed: Apply an object with a / in its name; verify the API returns a clear validation error rather than silently storing an
  unreachable object.

  ---
  9. Any Spec Field Named *_object or *_objects Is Silently Extracted

  File: handlers/apply.rs:325 (extract_navigation_properties)

  Nav-prop extraction is purely convention-based (field name suffix). This happens before schema validation, so the schema never sees
  those fields. Combined with deny_unknown_fields schemas: a client can attach any _object-suffixed field to any object, have it
  extracted, and attempt to apply it as a separate object — without the outer object's schema rejecting the unknown field.

  If extraction fails to deserialize the embedded value as ObjectAny, it is silently dropped with no error returned to the caller. This
  makes debugging difficult and hides malformed inputs.

  Test needed: Submit an object with an unknown _object field containing an invalid payload; verify the API returns an error rather than
  silently ignoring it.

  ---
  10. No Request Size Limit

  No mention of body size limits in any handler or middleware configuration. A single /apply request carrying a deeply nested nav-prop
  graph, or an array with thousands of objects, will be parsed entirely into memory before any validation runs. Combined with the FK graph
   walk (one DB round-trip per FK target), a crafted payload can cause unbounded memory use and database load.

  Test needed: Submit a list with a very large number of objects or deeply nested nav-props; verify the server enforces a limit and
  returns an appropriate error.

  ---
  Summary Table

  ┌─────┬──────────┬──────────────────────┬───────────────────────────────────────┐
  │  #  │ Severity │       Category       │                File(s)                │
  ├─────┼──────────┼──────────────────────┼───────────────────────────────────────┤
  │ 1   │ Critical │ Auth                 │ jwt_service.rs, middleware.rs         │
  ├─────┼──────────┼──────────────────────┼───────────────────────────────────────┤
  │ 2   │ Critical │ AuthZ                │ handlers/apply.rs, handlers/delete.rs │
  ├─────┼──────────┼──────────────────────┼───────────────────────────────────────┤
  │ 3   │ High     │ AuthZ bypass         │ controllers.rs, handlers/apply.rs     │
  ├─────┼──────────┼──────────────────────┼───────────────────────────────────────┤
  │ 4   │ High     │ Privilege escalation │ handlers/apply.rs                     │
  ├─────┼──────────┼──────────────────────┼───────────────────────────────────────┤
  │ 5   │ High     │ Auth                 │ controllers.rs                        │
  ├─────┼──────────┼──────────────────────┼───────────────────────────────────────┤
  │ 6   │ High     │ AuthZ                │ handlers/delete.rs, cache.rs          │
  ├─────┼──────────┼──────────────────────┼───────────────────────────────────────┤
  │ 7   │ Medium   │ Availability         │ cache.rs                              │
  ├─────┼──────────┼──────────────────────┼───────────────────────────────────────┤
  │ 8   │ Medium   │ Data integrity       │ handlers/apply.rs, rbac/helpers.rs    │
  ├─────┼──────────┼──────────────────────┼───────────────────────────────────────┤
  │ 9   │ Medium   │ Input validation     │ handlers/apply.rs                     │
  ├─────┼──────────┼──────────────────────┼───────────────────────────────────────┤
  │ 10  │ Medium   │ Availability         │ All handlers                          │
  └─────┴──────────┴──────────────────────┴───────────────────────────────────────┘
