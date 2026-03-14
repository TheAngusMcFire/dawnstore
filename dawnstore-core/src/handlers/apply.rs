use std::collections::HashSet;
use std::sync::Arc;

use dawnstore_lib::{ListOfObjects, ObjectAny, ReturnObject};

use crate::abstractions::{
    ForeignKeyBehaviour, ForeignKeyType, NewDawnStoreBackend, ObjectRelation,
    RawForeignKeyConstraint,
};
use crate::cache::DawnstoreCache;
use crate::error::DawnStoreError;
use crate::rbac::cache::{EffectivePermissions, GrantedScope, Verb};
use crate::rbac::helpers::object_string_id;
use crate::rbac::middleware::Claims;

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

/// Returns `true` if `perms` grants `verb` on `(kind, name)`.
/// Api-version filtering is intentionally omitted here; roles are matched solely
/// by verb + kind + name at the apply-handler level.
fn has_permission(perms: &EffectivePermissions, verb: Verb, kind: &str, name: &str) -> bool {
    let check = |scope: &GrantedScope| {
        scope.verbs.contains(&verb)
            && scope.kinds.iter().any(|k| k == "*" || k == kind)
            && scope.names.as_ref().map_or(true, |names| names.iter().any(|n| n == name))
    };
    perms.namespaced.iter().any(check) || perms.global.iter().any(check)
}

/// Verify that `caller` holds `verb` permission on `(namespace, kind, name)`.
///
/// When `caller` is `None` (unauthenticated / superadmin path) the check is
/// skipped and `Ok(())` is returned immediately.
///
/// Otherwise the permission cache is consulted first. On a cache miss the
/// full permission cache is rebuilt from the backend, and the check is
/// re-evaluated. Returns [`DawnStoreError::Forbidden`] if the caller lacks
/// the required permission.
async fn check_permission<B: NewDawnStoreBackend>(
    cache: &DawnstoreCache,
    backend: &B,
    caller: Option<&Claims>,
    verb: Verb,
    _namespace: &str,
    kind: &str,
    name: &str,
) -> Result<(), DawnStoreError> {
    let Some(caller) = caller else {
        return Ok(()); // unauthenticated / superadmin path: skip all checks
    };

    // Fast path: check the in-memory cache.
    if let Some(perms) = cache.get_permissions(&caller.namespace, &caller.sub) {
        return if has_permission(&perms, verb, kind, name) {
            Ok(())
        } else {
            Err(DawnStoreError::Forbidden)
        };
    }

    // Cache miss: rebuild the full permission cache from the backend, then
    // re-evaluate. The SA will have empty permissions if not in any binding.
    cache.init_permission(backend).await?;
    let perms = cache.get_permissions(&caller.namespace, &caller.sub).unwrap_or_default();
    if has_permission(&perms, verb, kind, name) {
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

/// Walk the full FK graph reachable from `seed_objects` using an iterative work queue.
///
/// Seeds the queue with all top-level objects being applied. For each object
/// popped from the queue:
/// 1. Skip if already visited (prevents cycles and redundant work).
/// 2. Look up FK constraints for that object's `(api_version, kind)` from cache.
/// 3. For each constraint call [`extract_fk_values`] to get resolved string IDs.
/// 4. For each target string ID:
///    a. Check `Get` permission for `caller` via [`check_permission`].
///    b. Fetch the target from `backend` via [`NewDawnStoreBackend::get_object`].
///    c. If the target is missing and the FK is required → return an error.
///    d. Record an [`ObjectRelation`] edge for the upsert step.
///    e. If not yet visited, push the fetched target onto the queue so its own
///       FK constraints are walked in the next iteration (handles arbitrary nesting
///       of navigation properties such as `Container.parent_object`).
///
/// Returns all [`ObjectRelation`] edges collected during the walk.
async fn walk_foreign_key_graph<B: NewDawnStoreBackend>(
    backend: &B,
    cache: &DawnstoreCache,
    caller: Option<&Claims>,
    seed_objects: &[ObjectAny],
) -> Result<Vec<ObjectRelation>, DawnStoreError> {
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

                // Fetch the target object from the backend.
                let target = backend.get_object(target_ns, target_kind, target_name).await?;

                match target {
                    None => {
                        // Missing target is only an error for required FK types.
                        if matches!(
                            constraint.ty,
                            ForeignKeyType::One | ForeignKeyType::OneOrMany
                        ) {
                            return Err(DawnStoreError::ObjectValidationForeignKeyNotFound {
                                api_version: constraint.api_version.clone(),
                                kind: constraint.kind.clone(),
                                name: obj.name.clone(),
                                value: target_id,
                            });
                        }
                    }
                    Some(target_obj) => {
                        // Record the FK relation edge.
                        relations.push(ObjectRelation {
                            object_string_id: string_id.clone(),
                            fk_constraint_id: constraint.id,
                            target_string_id: target_id.clone(),
                        });

                        // Push the target onto the stack for its own FK constraints
                        // to be walked, enabling arbitrary-depth nesting.
                        if !visited.contains(&target_id) {
                            let target_raw = serde_json::to_value(&target_obj)
                                .expect("ReturnObject always serializes");
                            stack.push(target_raw);
                        }
                    }
                }
            }
        }
    }

    Ok(relations)
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Apply one or more objects from a raw JSON payload.
///
/// `input` may be a single object, an array of objects, or a `ListOfObjects`
/// wrapper. `caller` is `None` when the request is unauthenticated (superadmin
/// path); all permission checks are skipped in that case.
///
/// See the module-level doc comment for the full step-by-step description.
pub async fn apply<B: NewDawnStoreBackend>(
    backend: &B,
    cache: &DawnstoreCache,
    caller: Option<&Claims>,
    input: serde_json::Value,
) -> Result<Vec<ReturnObject<serde_json::Value>>, DawnStoreError> {
    // Step 1: normalise the raw payload into a flat list of objects.
    let objects = parse_input(input)?;

    for obj in &objects {
        let namespace = obj.namespace.as_deref().unwrap_or("default");
        let kind = obj.kind.as_deref().ok_or(DawnStoreError::KindMissingInObject)?;
        let api_version =
            obj.api_version.as_deref().ok_or(DawnStoreError::ApiVersionMissingInObject)?;

        // Step 2: permission check (Apply) — fail fast before any heavier work.
        check_permission(cache, backend, caller, Verb::Apply, namespace, kind, &obj.name).await?;

        // Step 3: schema validation against the cached JSON schema validator.
        validate_schema(cache, api_version, kind, &obj.name, &obj.spec).await?;
    }

    // Steps 4 + 5: iterative FK graph walk — resolves, existence-checks, and
    // Get-permission-checks every FK target to arbitrary nesting depth.
    let relations = walk_foreign_key_graph(backend, cache, caller, &objects).await?;

    // Step 6: upsert all objects and reconcile the relations table in one transaction.
    backend.upsert_objects(objects, relations).await
}
