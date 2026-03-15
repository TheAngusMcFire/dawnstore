use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use dawnstore_lib::{ListOfObjects, ObjectAny, ReturnObject};

use crate::abstractions::{
    ForeignKeyBehaviour, ForeignKeyType, DawnstoreBackend, ObjectRelation,
    RawForeignKeyConstraint,
};
use crate::cache::DawnstoreCache;
use crate::error::DawnStoreError;
use crate::cache::{EffectivePermissions, GrantedScope, Verb, is_superadmin};
use crate::rbac::constants::{
    KIND_GLOBAL_ROLE, KIND_GLOBAL_ROLE_BINDING, KIND_NAMESPACE, KIND_ROLE, KIND_ROLE_BINDING,
    SYSTEM_NAMESPACE,
};
use crate::rbac::helpers::object_string_id;
use crate::rbac::middleware::Claims;
use crate::rbac::models::{GlobalRole, GlobalRoleBinding, PolicyRule, Role, RoleBinding};

// ── Private helpers ───────────────────────────────────────────────────────────

/// Parse a raw JSON payload into a flat, normalised list of objects.
///
/// Handles three input shapes:
/// - JSON array → each element is an object.
/// - Object with `kind = "List"` → extract the `list` field. If `object_kind`
///   and/or `object_api_version` are set on the wrapper, fill those values into
///   any item that is missing its own `kind` / `api_version` (implied properties).
/// - Any other single object → one-element list.
///
/// Returns [`DawnStoreError::InvalidRootInputObject`] for non-object/non-array
/// input, and [`DawnStoreError::InvalidInputObjectMissingKindField`] when the
/// `kind` field is absent on the root or the `list` field is missing from a List.
fn parse_input(input: serde_json::Value) -> Result<Vec<ObjectAny>, DawnStoreError> {
    if let Some(arr) = input.as_array() {
        arr.iter()
            .map(|v| serde_json::from_value(v.clone()).map_err(DawnStoreError::from))
            .collect()
    } else if input.is_object() {
        let kind = input.get("kind").and_then(|v| v.as_str());
        if kind == Some("List") {
            // Ensure the list field is present before full deserialization.
            if input.get("list").is_none() {
                return Err(DawnStoreError::InvalidInputObjectMissingListFieldOfList);
            }
            let list_obj: ListOfObjects = serde_json::from_value(input)?;
            let object_kind = list_obj.object_kind.clone();
            let object_api_version = list_obj.object_api_version.clone();
            // Apply implied properties: fill kind/api_version from the wrapper
            // onto any item that does not already have its own value.
            let mut items = list_obj.list;
            for item in &mut items {
                if item.kind.is_none() {
                    item.kind = object_kind.clone();
                }
                if item.api_version.is_none() {
                    item.api_version = object_api_version.clone();
                }
            }
            Ok(items)
        } else if kind.is_none() {
            Err(DawnStoreError::InvalidInputObjectMissingKindField)
        } else {
            Ok(vec![serde_json::from_value(input)?])
        }
    } else {
        Err(DawnStoreError::InvalidRootInputObject)
    }
}

/// Returns `true` if `perms` grants `verb` on `(target_namespace, kind, name)`.
///
/// Namespace-scoped grants (from `RoleBinding`s, stored in `perms.namespaced`) are
/// only honoured when `caller_namespace == target_namespace`. Global grants (from
/// `GlobalRoleBinding`s) apply regardless of namespace.
fn has_permission(
    perms: &EffectivePermissions,
    verb: Verb,
    caller_namespace: &str,
    target_namespace: &str,
    kind: &str,
    name: &str,
) -> bool {
    let check = |scope: &GrantedScope| {
        scope.verbs.contains(&verb)
            && scope.kinds.iter().any(|k| k == "*" || k == kind)
            && scope.names.as_ref().map_or(true, |names| names.iter().any(|n| n == name))
    };
    (caller_namespace == target_namespace && perms.namespaced.iter().any(check))
        || perms.global.iter().any(check)
}

/// Load the effective permissions for `caller` from the cache, rebuilding from
/// `backend` on a miss.
///
/// Reads the generation before detecting the miss so that concurrent callers
/// coalesce onto a single rebuild (see [`DawnstoreCache::init_permission`]).
async fn get_or_load_permissions<B: DawnstoreBackend>(
    cache: &DawnstoreCache,
    backend: &B,
    caller: &Claims,
) -> Result<EffectivePermissions, DawnStoreError> {
    if let Some(perms) = cache.get_permissions(&caller.namespace, &caller.sub) {
        return Ok(perms);
    }
    let miss_gen = cache.permission_generation();
    cache.init_permission(backend, miss_gen).await?;
    Ok(cache.get_permissions(&caller.namespace, &caller.sub).unwrap_or_default())
}

/// Verify that `caller` holds `verb` permission on `(namespace, kind, name)`.
///
/// When `caller` is `None` (unauthenticated / superadmin path) the check is
/// skipped and `Ok(())` is returned immediately.
///
/// Namespace-scoped grants (from `RoleBinding`s) are only honoured when the
/// target `namespace` matches the caller's own namespace. Global grants apply
/// across all namespaces.
///
/// Returns [`DawnStoreError::Forbidden`] if the caller lacks the required permission.
async fn check_permission<B: DawnstoreBackend>(
    cache: &DawnstoreCache,
    backend: &B,
    caller: Option<&Claims>,
    verb: Verb,
    namespace: &str,
    kind: &str,
    name: &str,
) -> Result<(), DawnStoreError> {
    let Some(caller) = caller else {
        return Ok(()); // unauthenticated / superadmin path: skip all checks
    };
    if is_superadmin(caller) {
        return Ok(()); // superadmin bypasses all permission checks
    }
    let perms = get_or_load_permissions(cache, backend, caller).await?;
    if has_permission(&perms, verb, &caller.namespace, namespace, kind, name) {
        Ok(())
    } else {
        Err(DawnStoreError::Forbidden)
    }
}

/// Validate `spec` against the compiled JSON schema for `(api_version, kind)`.
///
/// Looks up the pre-compiled [`jsonschema::Validator`] from the schema cache.
/// Returns [`DawnStoreError::NoSchemaForObjectFound`] on a cache miss (schema
/// must be registered before objects of that kind can be applied), or a
/// [`DawnStoreError::ObjectValidationError`] if the spec does not conform.
async fn validate_schema(
    cache: &DawnstoreCache,
    api_version: &str,
    kind: &str,
    name: &str,
    spec: &serde_json::Value,
) -> Result<(), DawnStoreError> {
    let validator = cache
        .get_schema(api_version, kind)
        .await
        .ok_or_else(|| DawnStoreError::NoSchemaForObjectFound {
            api_version: api_version.to_string(),
            kind: kind.to_string(),
        })?;
    if let Err(e) = validator.validate(spec) {
        return Err(DawnStoreError::ObjectValidationError {
            api_version: api_version.to_string(),
            kind: kind.to_string(),
            name: name.to_string(),
            validation_error: e.to_owned(),
        });
    }
    Ok(())
}

/// Walk a dot-separated `path` inside `value`, returning the nested value or
/// `None` if any segment is missing.
fn walk_path<'a>(value: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    let mut current = value;
    for segment in path.split('.') {
        current = current.get(segment)?;
    }
    Some(current)
}

/// Resolve a raw FK string to a canonical `namespace/kind/name` string ID.
/// Fills in `object_ns` and `default_kind` for 1- or 2-segment values.
/// Returns `Err(())` if the segment count is not 1, 2, or 3 (i.e. > 3).
fn resolve_fk_string(value: &str, object_ns: &str, default_kind: Option<&str>) -> Result<String, ()> {
    let parts: Vec<&str> = value.split('/').collect();
    match parts.as_slice() {
        [name] => {
            let kind = default_kind.ok_or(())?;
            Ok(object_string_id(object_ns, kind, name))
        }
        [kind, name] => Ok(object_string_id(object_ns, kind, name)),
        [namespace, kind, name] => Ok(object_string_id(namespace, kind, name)),
        _ => Err(()),
    }
}

/// Extract and validate all FK string values from `spec` for a single constraint.
///
/// Walks `constraint.key_path` (dot-separated) inside `spec` to locate the FK
/// field. Validates:
/// - Presence: required FK types (`One`, `OneOrMany`) must not be missing or null.
/// - Format: each value must have 1, 2, or 3 `/`-separated components; other
///   counts are invalid.
/// - Kind constraint: if `constraint.foreign_key_kind` is set, the kind segment
///   of every value must match it exactly.
///
/// Returns the resolved canonical string IDs (`namespace/kind/name`), using
/// `object_ns` and `constraint.foreign_key_kind` to fill in missing segments.
/// Optional FK types that are absent or null return an empty `Vec`.
fn extract_fk_values(
    spec: &serde_json::Value,
    constraint: &RawForeignKeyConstraint,
    object_ns: &str,
    object_name: &str,
) -> Result<Vec<String>, DawnStoreError> {
    let field = walk_path(spec, &constraint.key_path);
    let is_absent = field.map(|v| v.is_null()).unwrap_or(true);

    if is_absent {
        return match constraint.ty {
            ForeignKeyType::One | ForeignKeyType::OneOrMany => {
                Err(DawnStoreError::ObjectValidationMissingForeignKeyEntry {
                    api_version: constraint.api_version.clone(),
                    kind: constraint.kind.clone(),
                    name: object_name.to_string(),
                    foreign_key_path: constraint.key_path.clone(),
                    foreign_key_type: constraint.ty,
                })
            }
            ForeignKeyType::OneOptional | ForeignKeyType::NoneOrMany => Ok(vec![]),
        };
    }

    let field = field.unwrap();

    // Collect the raw string values from the field (single or array).
    let raw_values: Vec<&str> = match constraint.ty {
        ForeignKeyType::One | ForeignKeyType::OneOptional => {
            let s = field.as_str().ok_or_else(|| {
                DawnStoreError::ObjectValidationWrongForeignKeyEntryFormat {
                    api_version: constraint.api_version.clone(),
                    kind: constraint.kind.clone(),
                    name: object_name.to_string(),
                    foreign_key_path: constraint.key_path.clone(),
                    foreign_key_type: constraint.ty,
                    value: field.to_string(),
                }
            })?;
            vec![s]
        }
        ForeignKeyType::OneOrMany | ForeignKeyType::NoneOrMany => {
            let arr = field.as_array().ok_or_else(|| {
                DawnStoreError::ObjectValidationWrongForeignKeyEntryFormat {
                    api_version: constraint.api_version.clone(),
                    kind: constraint.kind.clone(),
                    name: object_name.to_string(),
                    foreign_key_path: constraint.key_path.clone(),
                    foreign_key_type: constraint.ty,
                    value: field.to_string(),
                }
            })?;
            arr.iter()
                .map(|v| {
                    v.as_str().ok_or_else(|| {
                        DawnStoreError::ObjectValidationWrongForeignKeyEntryFormat {
                            api_version: constraint.api_version.clone(),
                            kind: constraint.kind.clone(),
                            name: object_name.to_string(),
                            foreign_key_path: constraint.key_path.clone(),
                            foreign_key_type: constraint.ty,
                            value: v.to_string(),
                        }
                    })
                })
                .collect::<Result<Vec<_>, _>>()?
        }
    };

    let default_kind = constraint.foreign_key_kind.as_deref();
    let mut result = Vec::new();

    for raw in raw_values {
        // Validate format (1–3 segments) and optional kind constraint.
        let parts: Vec<&str> = raw.split('/').collect();
        let kind_in_value: Option<&str> = match parts.as_slice() {
            [_name] => None,
            [kind, _name] => Some(kind),
            [_ns, kind, _name] => Some(kind),
            _ => {
                return Err(DawnStoreError::ObjectValidationWrongForeignKeyEntryFormat {
                    api_version: constraint.api_version.clone(),
                    kind: constraint.kind.clone(),
                    name: object_name.to_string(),
                    foreign_key_path: constraint.key_path.clone(),
                    foreign_key_type: constraint.ty,
                    value: raw.to_string(),
                });
            }
        };
        if let (Some(expected), Some(actual)) = (default_kind, kind_in_value) {
            if actual != expected {
                return Err(DawnStoreError::ObjectValidationWrongForeignKeyEntryKind {
                    api_version: constraint.api_version.clone(),
                    kind: constraint.kind.clone(),
                    name: object_name.to_string(),
                    foreign_key_path: constraint.key_path.clone(),
                    foreign_key_type: constraint.ty,
                    value: raw.to_string(),
                });
            }
        }

        let resolved = resolve_fk_string(raw, object_ns, default_kind).map_err(|_| {
            DawnStoreError::ObjectValidationWrongForeignKeyEntryFormat {
                api_version: constraint.api_version.clone(),
                kind: constraint.kind.clone(),
                name: object_name.to_string(),
                foreign_key_path: constraint.key_path.clone(),
                foreign_key_type: constraint.ty,
                value: raw.to_string(),
            }
        })?;
        result.push(resolved);
    }

    Ok(result)
}

/// Extract embedded navigation-property objects from a list of objects.
///
/// Navigation properties are spec fields whose name ends with `_object` (a single
/// embedded `ReturnObject`) or `_objects` (an array of them). Each embedded value
/// is removed from the parent's spec and returned as a separate `ObjectAny` so it
/// can be validated, permission-checked, and upserted alongside the parent.
///
/// Returning the extracted objects separately lets the caller prepend them to the
/// batch so they are available as FK targets when the parent object is processed.
fn extract_navigation_properties(objects: Vec<ObjectAny>) -> (Vec<ObjectAny>, Vec<ObjectAny>) {
    let mut cleaned = Vec::with_capacity(objects.len());
    let mut extracted = Vec::new();

    for mut obj in objects {
        if let serde_json::Value::Object(ref mut map) = obj.spec {
            let nav_keys: Vec<String> = map
                .keys()
                .filter(|k| k.ends_with("_object") || k.ends_with("_objects"))
                .cloned()
                .collect();

            for key in nav_keys {
                let val = match map.remove(&key) {
                    Some(v) => v,
                    None => continue,
                };
                if key.ends_with("_objects") {
                    if let Some(arr) = val.as_array() {
                        for item in arr {
                            if let Ok(embedded) = serde_json::from_value::<ObjectAny>(item.clone()) {
                                extracted.push(embedded);
                            }
                        }
                    }
                } else if !val.is_null() {
                    if let Ok(embedded) = serde_json::from_value::<ObjectAny>(val) {
                        extracted.push(embedded);
                    }
                }
            }
        }
        cleaned.push(obj);
    }

    (cleaned, extracted)
}

/// Walk the full FK graph reachable from `seed_objects` using an iterative work queue.
///
/// Seeds the queue with all top-level objects being applied. For each object
/// popped from the queue:
/// 1. Skip if already visited (prevents cycles and redundant work).
/// 2. Look up FK constraints for that object's `(api_version, kind)` from cache.
/// 3. For each constraint call [`extract_fk_values`] to get resolved string IDs.
/// 4. For each target string ID:
///    a. Check `Get` permission for `caller` via [`check_permission`].
///    b. Fetch the target from `backend` via [`DawnstoreBackend::get_object`].
///    c. If the target is missing and the FK is required → return an error.
///    d. Record an [`ObjectRelation`] edge for the upsert step.
///    e. If not yet visited, push the fetched target onto the queue so its own
///       FK constraints are walked in the next iteration (handles arbitrary nesting
///       of navigation properties such as `Container.parent_object`).
///
/// Returns all [`ObjectRelation`] edges collected during the walk.
async fn walk_foreign_key_graph<B: DawnstoreBackend>(
    backend: &B,
    cache: &DawnstoreCache,
    caller: Option<&Claims>,
    seed_objects: &[ObjectAny],
) -> Result<Vec<ObjectRelation>, DawnStoreError> {
    // Build an index of all objects in this batch by string ID so that FK
    // targets within the same batch are treated as already-existing without
    // requiring a database round-trip that would fail (they haven't been
    // upserted yet when the FK walk runs).
    let batch_index: HashMap<String, serde_json::Value> = seed_objects
        .iter()
        .filter_map(|obj| {
            let ns = obj.namespace.as_deref().unwrap_or("default");
            let kind = obj.kind.as_deref()?;
            let sid = object_string_id(ns, kind, &obj.name);
            let val = serde_json::to_value(obj).ok()?;
            Some((sid, val))
        })
        .collect();

    // Use a Vec as a work stack (DFS). Serialise ObjectAny to serde_json::Value
    // to avoid needing Clone on the non-Clone ObjectAny type.
    let mut stack: Vec<serde_json::Value> = seed_objects
        .iter()
        .map(|obj| serde_json::to_value(obj).expect("ObjectAny always serializes"))
        .collect();

    let mut visited: HashSet<String> = HashSet::new();
    let mut relations: Vec<ObjectRelation> = Vec::new();

    while let Some(raw) = stack.pop() {
        let obj: ObjectAny = serde_json::from_value(raw)?;
        let namespace = obj.namespace.as_deref().unwrap_or("default");
        let kind = obj.kind.as_deref().ok_or(DawnStoreError::KindMissingInObject)?;
        let api_version =
            obj.api_version.as_deref().ok_or(DawnStoreError::ApiVersionMissingInObject)?;
        let string_id = object_string_id(namespace, kind, &obj.name);

        // Skip if already visited (prevents cycles and redundant work).
        if !visited.insert(string_id.clone()) {
            continue;
        }

        // Look up FK constraints for this object's (api_version, kind).
        // If there are no constraints registered, skip quietly.
        let constraints: Arc<Vec<_>> =
            cache.get_foreign_keys(api_version, kind).await.unwrap_or_else(|| Arc::new(vec![]));

        for constraint in constraints.iter() {
            // Ignore-behaviour constraints are not resolved; they are used only
            // to declare that the FK field exists, not to enforce its presence.
            if matches!(constraint.behaviour, ForeignKeyBehaviour::Ignore) {
                continue;
            }

            let target_ids =
                extract_fk_values(&obj.spec, constraint, namespace, &obj.name)?;

            for target_id in target_ids {
                // Parse namespace/kind/name from the resolved string ID.
                let parts: Vec<&str> = target_id.splitn(3, '/').collect();
                let (target_ns, target_kind, target_name) = match parts.as_slice() {
                    [ns, k, n] => (*ns, *k, *n),
                    _ => continue,
                };

                // Permission check: caller must have Get on the FK target.
                check_permission(
                    cache,
                    backend,
                    caller,
                    Verb::Get,
                    target_ns,
                    target_kind,
                    target_name,
                )
                .await?;

                // Fetch the target: first check the current batch (objects being
                // applied together may reference each other), then the database.
                let target_raw: Option<serde_json::Value> =
                    if let Some(v) = batch_index.get(&target_id) {
                        Some(v.clone())
                    } else {
                        backend
                            .get_object(target_ns, target_kind, target_name)
                            .await?
                            .map(|o| {
                                serde_json::to_value(&o).expect("ReturnObject always serializes")
                            })
                    };

                match target_raw {
                    None => {
                        // A specified FK value must point to an existing object or
                        // another object in the same batch being applied.
                        return Err(DawnStoreError::ObjectValidationForeignKeyNotFound {
                            api_version: constraint.api_version.clone(),
                            kind: constraint.kind.clone(),
                            name: obj.name.clone(),
                            value: target_id,
                        });
                    }
                    Some(target_val) => {
                        // Record the FK relation edge.
                        relations.push(ObjectRelation {
                            object_string_id: string_id.clone(),
                            fk_constraint_id: constraint.id,
                            target_string_id: target_id.clone(),
                        });

                        // Push the target onto the stack for its own FK constraints
                        // to be walked, enabling arbitrary-depth nesting.
                        if !visited.contains(&target_id) {
                            stack.push(target_val);
                        }
                    }
                }
            }
        }
    }

    Ok(relations)
}

// ── Privilege-escalation prevention ───────────────────────────────────────────

/// Returns `true` if `caller_perms` covers every (verb, kind) pair in `rule`,
/// meaning the caller is entitled to grant those permissions to others.
///
/// For namespace-scoped roles (`global_only = false`) both namespaced and global
/// grants are accepted, subject to the usual namespace equality constraint.
/// For cluster-wide roles (`global_only = true`) only global grants are accepted,
/// because a namespace-scoped grant cannot confer cross-namespace power.
///
/// The names restriction is handled conservatively: a scope that is restricted to
/// a specific set of names can only satisfy a rule whose names restriction is equal
/// or narrower. An unrestricted scope (names = `None`) satisfies any rule.
fn caller_can_grant_rule(
    perms: &EffectivePermissions,
    rule: &PolicyRule,
    caller_ns: &str,
    target_ns: &str,
    global_only: bool,
) -> bool {
    for verb_str in &rule.verbs {
        let Some(verb) = Verb::from_str(verb_str) else {
            return false; // unknown verb → conservatively deny
        };
        for rule_kind in &rule.kinds {
            let scope_covers = |scope: &GrantedScope| -> bool {
                if !scope.verbs.contains(&verb) {
                    return false;
                }
                // If the rule grants "*" (all kinds), the caller's grant must also
                // be "*". If the rule names a specific kind, "*" or that kind suffices.
                let kind_ok = if rule_kind == "*" {
                    scope.kinds.iter().any(|k| k == "*")
                } else {
                    scope.kinds.iter().any(|k| k == "*" || k == rule_kind)
                };
                if !kind_ok {
                    return false;
                }
                // The caller's names restriction must be at least as permissive as
                // the rule's restriction. If the caller is restricted to a set of
                // names, the rule must also be restricted and must be a subset.
                match (&scope.names, &rule.names) {
                    (None, _) => true,
                    (Some(_), None) => false,
                    (Some(caller_names), Some(rule_names)) => {
                        rule_names.iter().all(|rn| caller_names.iter().any(|cn| cn == rn))
                    }
                }
            };
            let allowed = if global_only {
                perms.global.iter().any(scope_covers)
            } else {
                (caller_ns == target_ns && perms.namespaced.iter().any(scope_covers))
                    || perms.global.iter().any(scope_covers)
            };
            if !allowed {
                return false;
            }
        }
    }
    true
}

/// Retrieve the spec JSON for an object by its canonical `namespace/kind/name`
/// string ID. Checks `spec_cache` first (populated with batch objects and
/// previously fetched specs); on a miss, fetches from `backend` and caches the
/// result so subsequent lookups for the same SID are free.
async fn get_or_fetch_spec<B: DawnstoreBackend>(
    spec_cache: &mut HashMap<String, serde_json::Value>,
    backend: &B,
    sid: &str,
) -> Result<Option<serde_json::Value>, DawnStoreError> {
    if let Some(spec) = spec_cache.get(sid) {
        return Ok(Some(spec.clone()));
    }
    let parts: Vec<&str> = sid.splitn(3, '/').collect();
    match parts.as_slice() {
        [ns, kind, name] => {
            if let Some(obj) = backend.get_object(ns, kind, name).await? {
                spec_cache.insert(sid.to_string(), obj.spec.clone());
                Ok(Some(obj.spec))
            } else {
                Ok(None)
            }
        }
        _ => Ok(None),
    }
}

/// Enforce that `caller` is not escalating privileges through RBAC objects.
///
/// A principal must not be able to grant permissions they do not themselves hold.
/// This check applies to all four RBAC kinds:
/// - `Role` / `GlobalRole`: every rule in the spec must be within the caller's own grants.
/// - `RoleBinding` / `GlobalRoleBinding`: the referenced role is fetched (from
///   the current batch or the backend) and its rules are checked against the caller.
///
/// The check is skipped entirely for unauthenticated callers and the system
/// superadmin, which are both unrestricted.
async fn check_rbac_escalation<B: DawnstoreBackend>(
    backend: &B,
    cache: &DawnstoreCache,
    caller: Option<&Claims>,
    objects: &[ObjectAny],
) -> Result<(), DawnStoreError> {
    let Some(caller) = caller else {
        return Ok(()); // unauthenticated / superadmin path
    };
    if is_superadmin(caller) {
        return Ok(());
    }

    // Early exit: skip the check entirely if no RBAC objects are present.
    // This is the common case for non-RBAC apply requests.
    let has_rbac = objects.iter().any(|obj| {
        matches!(
            obj.kind.as_deref(),
            Some(KIND_ROLE)
                | Some(KIND_GLOBAL_ROLE)
                | Some(KIND_ROLE_BINDING)
                | Some(KIND_GLOBAL_ROLE_BINDING)
        )
    });
    if !has_rbac {
        return Ok(());
    }

    let perms = get_or_load_permissions(cache, backend, caller).await?;

    // Single pass: resolve each kind alias once and index Role/GlobalRole specs.
    // Only those two kinds are ever referenced by bindings, so only their specs
    // need to be in the cache. Backend fetches for the same role are deduplicated
    // via the same cache in `get_or_fetch_spec`.
    let mut resolved: Vec<(String, String)> = Vec::with_capacity(objects.len());
    let mut spec_cache: HashMap<String, serde_json::Value> = HashMap::new();
    for obj in objects {
        let ns = obj.namespace.as_deref().unwrap_or("default").to_string();
        let raw_kind = obj.kind.as_deref().unwrap_or("");
        let canonical_kind =
            cache.resolve_kind(raw_kind).await.unwrap_or_else(|| raw_kind.to_string());
        if matches!(canonical_kind.as_str(), KIND_ROLE | KIND_GLOBAL_ROLE) {
            let sid = object_string_id(&ns, &canonical_kind, &obj.name);
            spec_cache.insert(sid, obj.spec.clone());
        }
        resolved.push((ns, canonical_kind));
    }

    for (obj, (ns, canonical_kind)) in objects.iter().zip(resolved.iter()) {
        match canonical_kind.as_str() {
            KIND_ROLE => {
                let role: Role = serde_json::from_value(obj.spec.clone())?;
                for rule in &role.rules {
                    if !caller_can_grant_rule(&perms, rule, &caller.namespace, ns, false) {
                        return Err(DawnStoreError::Forbidden);
                    }
                }
            }
            KIND_GLOBAL_ROLE => {
                let role: GlobalRole = serde_json::from_value(obj.spec.clone())?;
                for rule in &role.rules {
                    // Global roles apply across all namespaces; only global grants satisfy.
                    if !caller_can_grant_rule(&perms, rule, &caller.namespace, ns, true) {
                        return Err(DawnStoreError::Forbidden);
                    }
                }
            }
            KIND_ROLE_BINDING => {
                let binding: RoleBinding = serde_json::from_value(obj.spec.clone())?;
                // Schema validation guarantees the FK string has 1–3 segments; the
                // map_err converts the impossible error to an InternalServerError.
                let role_sid = resolve_fk_string(&binding.role, ns, Some(KIND_ROLE))
                    .map_err(|_| DawnStoreError::InternalServerError(
                        format!("malformed role FK in validated RoleBinding spec: {}", binding.role),
                    ))?;
                if let Some(role_spec) =
                    get_or_fetch_spec(&mut spec_cache, backend, &role_sid).await?
                {
                    let role: Role = serde_json::from_value(role_spec)?;
                    for rule in &role.rules {
                        if !caller_can_grant_rule(&perms, rule, &caller.namespace, ns, false) {
                            return Err(DawnStoreError::Forbidden);
                        }
                    }
                }
                // If the role is not found, the FK walk (next step) returns a proper error.
            }
            KIND_GLOBAL_ROLE_BINDING => {
                let binding: GlobalRoleBinding = serde_json::from_value(obj.spec.clone())?;
                let role_sid = resolve_fk_string(&binding.role, ns, Some(KIND_GLOBAL_ROLE))
                    .map_err(|_| DawnStoreError::InternalServerError(
                        format!("malformed role FK in validated GlobalRoleBinding spec: {}", binding.role),
                    ))?;
                if let Some(role_spec) =
                    get_or_fetch_spec(&mut spec_cache, backend, &role_sid).await?
                {
                    let role: GlobalRole = serde_json::from_value(role_spec)?;
                    for rule in &role.rules {
                        if !caller_can_grant_rule(&perms, rule, &caller.namespace, ns, true) {
                            return Err(DawnStoreError::Forbidden);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    Ok(())
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Apply one or more objects from a raw JSON payload.
///
/// `input` may be a single object, an array of objects, or a `ListOfObjects`
/// wrapper. `caller` is `None` when the request is unauthenticated (superadmin
/// path); all permission checks are skipped in that case.
///
/// See the module-level doc comment for the full step-by-step description.
pub async fn apply<B: DawnstoreBackend>(
    backend: &B,
    cache: &DawnstoreCache,
    caller: Option<&Claims>,
    input: serde_json::Value,
) -> Result<Vec<ReturnObject<serde_json::Value>>, DawnStoreError> {
    // Step 1: normalise the raw payload into a flat list of objects.
    let raw_objects = parse_input(input)?;

    // Step 2: extract embedded navigation-property objects from the input.
    // Nav-prop fields (ending in `_object` / `_objects`) are removed from each
    // parent's spec and prepended to the batch so they are FK-available when
    // their parent is processed. They are validated and upserted alongside the
    // parent objects, so the caller only needs to send one request.
    let (parent_objects, nav_objects) = extract_navigation_properties(raw_objects);
    let objects: Vec<ObjectAny> = nav_objects.into_iter().chain(parent_objects).collect();

    // Step 3: reject any object whose name or namespace contains '/'.
    // A '/' in either field creates ambiguous string IDs (namespace/kind/name)
    // and breaks all FK shorthand formats that rely on splitting on '/'.
    for obj in &objects {
        if obj.name.contains('/') {
            return Err(DawnStoreError::InvalidObjectName(obj.name.clone()));
        }
        let ns = obj.namespace.as_deref().unwrap_or("default");
        if ns.contains('/') {
            return Err(DawnStoreError::InvalidObjectNamespace(ns.to_string()));
        }
    }

    // Step 4: enforce the namespace restriction on the full assembled batch,
    // including any objects that arrived via nav-prop embedding. `Namespace`
    // objects may only be created inside the `system` namespace; any attempt
    // to create one elsewhere — including through an embedded nav-prop that
    // bypassed the controller-level check — is rejected here.
    for obj in &objects {
        let ns = obj.namespace.as_deref().unwrap_or("default");
        let kind_raw = obj.kind.as_deref().unwrap_or("");
        if cache.resolve_kind(kind_raw).await.as_deref() == Some(KIND_NAMESPACE)
            && ns != SYSTEM_NAMESPACE
        {
            return Err(DawnStoreError::NamespaceCanOnlyBeCreatedInSystemNamespace(
                ns.to_string(),
            ));
        }
    }

    for obj in &objects {
        let namespace = obj.namespace.as_deref().unwrap_or("default");
        let kind = obj.kind.as_deref().ok_or(DawnStoreError::KindMissingInObject)?;
        let api_version =
            obj.api_version.as_deref().ok_or(DawnStoreError::ApiVersionMissingInObject)?;

        // Step 5: permission check (Apply) — fail fast before any heavier work.
        check_permission(cache, backend, caller, Verb::Apply, namespace, kind, &obj.name).await?;

        // Step 6: schema validation against the cached JSON schema validator.
        validate_schema(cache, api_version, kind, &obj.name, &obj.spec).await?;
    }

    // Step 7: privilege-escalation check — reject if any RBAC object in the batch
    // would grant permissions the caller does not themselves hold.
    check_rbac_escalation(backend, cache, caller, &objects).await?;

    // Steps 8 + 9: iterative FK graph walk — resolves, existence-checks, and
    // Get-permission-checks every FK target to arbitrary nesting depth.
    let relations = walk_foreign_key_graph(backend, cache, caller, &objects).await?;

    // Step 8: upsert all objects and reconcile the relations table in one transaction.
    let applied = backend.upsert_objects(objects, relations).await?;

    // Step 9: invalidate the RBAC permission cache for any applied RBAC resources so
    // that downstream Get / Apply / Delete checks reflect the change immediately.
    for obj in &applied {
        if matches!(
            obj.kind.as_str(),
            KIND_ROLE | KIND_GLOBAL_ROLE | KIND_ROLE_BINDING | KIND_GLOBAL_ROLE_BINDING
        ) {
            cache.invalidate_permissions(&object_string_id(&obj.namespace, &obj.kind, &obj.name));
        }
    }

    Ok(applied)
}
