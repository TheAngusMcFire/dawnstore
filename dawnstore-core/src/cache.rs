use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use tokio::sync::RwLock as TokioRwLock;

use crate::abstractions::{BackendGetObjectsFilter, NewDawnStoreBackend, RawForeignKeyConstraint};
use crate::error::DawnStoreError;
use crate::rbac::cache::{EffectivePermissions, GrantedScope, Verb};
use crate::rbac::constants::{
    KIND_GLOBAL_ROLE, KIND_GLOBAL_ROLE_BINDING, KIND_ROLE, KIND_ROLE_BINDING, KIND_SERVICE_ACCOUNT,
};
use crate::rbac::helpers::{object_string_id, schema_cache_key};
use crate::rbac::models::{GlobalRole, GlobalRoleBinding, Role, RoleBinding};

// ── Internal permission state ─────────────────────────────────────────────────

#[derive(Default)]
struct PermissionState {
    /// Maps `(namespace, sa_name)` → effective permission set.
    permissions: HashMap<(String, String), EffectivePermissions>,
    /// Maps an RBAC object string ID to the SA keys whose permissions it contributed to.
    resource_index: HashMap<String, HashSet<(String, String)>>,
}

// ── DawnstoreCache ────────────────────────────────────────────────────────────

/// Unified cache for schema validators, foreign key constraints, and RBAC permissions.
#[derive(Default)]
pub struct DawnstoreCache {
    schema: TokioRwLock<HashMap<String, Arc<jsonschema::Validator>>>,
    foreign_key: TokioRwLock<HashMap<String, Arc<Vec<RawForeignKeyConstraint>>>>,
    permission: RwLock<PermissionState>,
}

impl DawnstoreCache {
    // ── Init ──────────────────────────────────────────────────────────────────

    /// Initialise all three caches from `backend` in sequence.
    pub async fn init<B: NewDawnStoreBackend>(backend: &B) -> Result<Self, DawnStoreError> {
        let cache = Self::default();
        cache.init_schema(backend).await?;
        cache.init_foreign_key(backend).await?;
        cache.init_permission(backend).await?;
        Ok(cache)
    }

    /// Populate the schema cache from all schemas returned by `backend`.
    pub async fn init_schema<B: NewDawnStoreBackend>(
        &self,
        backend: &B,
    ) -> Result<(), DawnStoreError> {
        let schemas = backend.load_all_schemas().await?;
        let mut cache = self.schema.write().await;
        for schema in schemas {
            let key = schema_cache_key(&schema.api_version, &schema.kind);
            let value: serde_json::Value = serde_json::from_str(&schema.json_schema)?;
            let validator = Arc::new(jsonschema::validator_for(&value)?);
            cache.insert(key, validator);
        }
        Ok(())
    }

    /// Populate the FK cache from all constraints returned by `backend`.
    pub async fn init_foreign_key<B: NewDawnStoreBackend>(
        &self,
        backend: &B,
    ) -> Result<(), DawnStoreError> {
        let all_fks = backend.load_all_foreign_key_constraints().await?;
        let mut grouped: HashMap<String, Vec<RawForeignKeyConstraint>> = HashMap::new();
        for fk in all_fks {
            let key = schema_cache_key(&fk.api_version, &fk.kind);
            grouped.entry(key).or_default().push(fk);
        }
        let mut cache = self.foreign_key.write().await;
        for (key, constraints) in grouped {
            cache.insert(key, Arc::new(constraints));
        }
        Ok(())
    }

    /// Populate the permission cache from all RBAC objects returned by `backend`.
    pub async fn init_permission<B: NewDawnStoreBackend>(
        &self,
        backend: &B,
    ) -> Result<(), DawnStoreError> {
        let (permissions, resource_index) = build_full_permission_cache(backend).await?;
        let mut state = self.permission.write().unwrap();
        state.permissions = permissions;
        state.resource_index = resource_index;
        Ok(())
    }

    // ── Schema cache access ───────────────────────────────────────────────────

    /// Return the compiled validator for `api_version/kind`, or `None` on a cache miss.
    pub async fn get_schema(
        &self,
        api_version: &str,
        kind: &str,
    ) -> Option<Arc<jsonschema::Validator>> {
        let key = schema_cache_key(api_version, kind);
        self.schema.read().await.get(&key).cloned()
    }

    /// Insert a compiled validator into the schema cache.
    pub async fn insert_schema(
        &self,
        api_version: &str,
        kind: &str,
        validator: jsonschema::Validator,
    ) {
        let key = schema_cache_key(api_version, kind);
        self.schema.write().await.insert(key, Arc::new(validator));
    }

    // ── FK cache access ───────────────────────────────────────────────────────

    /// Return the FK constraints for `api_version/kind`, or `None` on a cache miss.
    pub async fn get_foreign_keys(
        &self,
        api_version: &str,
        kind: &str,
    ) -> Option<Arc<Vec<RawForeignKeyConstraint>>> {
        let key = schema_cache_key(api_version, kind);
        self.foreign_key.read().await.get(&key).cloned()
    }

    /// Insert FK constraints into the FK cache.
    pub async fn insert_foreign_keys(
        &self,
        api_version: &str,
        kind: &str,
        constraints: Vec<RawForeignKeyConstraint>,
    ) {
        let key = schema_cache_key(api_version, kind);
        self.foreign_key.write().await.insert(key, Arc::new(constraints));
    }

    // ── Permission cache access ───────────────────────────────────────────────

    /// Return the cached effective permissions for `(namespace, sa_name)`, if present.
    pub fn get_permissions(&self, namespace: &str, sa_name: &str) -> Option<EffectivePermissions> {
        let key = (namespace.to_string(), sa_name.to_string());
        self.permission.read().unwrap().permissions.get(&key).cloned()
    }

    /// Insert effective permissions for `(namespace, sa_name)` and record which
    /// RBAC object string IDs contributed, so they can be invalidated later.
    pub fn insert_permissions(
        &self,
        namespace: &str,
        sa_name: &str,
        perms: EffectivePermissions,
        contributing_ids: Vec<String>,
    ) {
        let key = (namespace.to_string(), sa_name.to_string());
        let mut state = self.permission.write().unwrap();
        state.permissions.insert(key.clone(), perms);
        for res_id in contributing_ids {
            state.resource_index.entry(res_id).or_default().insert(key.clone());
        }
    }

    /// Evict all permission cache entries derived from `rbac_object_string_id`.
    ///
    /// Call this after a successful apply/delete of a role, rolebinding,
    /// globalrole, or globalrolebinding.
    pub fn invalidate_permissions(&self, rbac_object_string_id: &str) {
        let mut state = self.permission.write().unwrap();
        if let Some(affected_keys) = state.resource_index.remove(rbac_object_string_id) {
            for key in affected_keys {
                state.permissions.remove(&key);
            }
        }
    }
}

// ── Permission cache helpers ──────────────────────────────────────────────────

/// Resolve a FK string to a canonical `namespace/kind/name` string ID using
/// [`object_string_id`], filling in missing segments from `default_ns` / `default_kind`.
fn resolve_fk(value: &str, default_ns: &str, default_kind: &str) -> String {
    let parts: Vec<&str> = value.splitn(3, '/').collect();
    match parts.as_slice() {
        [ns, kind, name] => object_string_id(ns, kind, name),
        [kind, name] => object_string_id(default_ns, kind, name),
        [name] => object_string_id(default_ns, default_kind, name),
        _ => value.to_string(),
    }
}

fn scopes_from_role(role: &Role) -> Vec<GrantedScope> {
    role.rules
        .iter()
        .map(|rule| GrantedScope {
            api_version: rule.api_version.clone(),
            kinds: rule.kinds.clone(),
            verbs: rule.verbs.iter().filter_map(|v| Verb::from_str(v)).collect(),
            names: rule.names.clone(),
        })
        .collect()
}

fn scopes_from_global_role(role: &GlobalRole) -> Vec<GrantedScope> {
    role.rules
        .iter()
        .map(|rule| GrantedScope {
            api_version: rule.api_version.clone(),
            kinds: rule.kinds.clone(),
            verbs: rule.verbs.iter().filter_map(|v| Verb::from_str(v)).collect(),
            names: rule.names.clone(),
        })
        .collect()
}

/// Build the full permission map by loading every RBAC object from `backend`.
async fn build_full_permission_cache<B: NewDawnStoreBackend>(
    backend: &B,
) -> Result<
    (
        HashMap<(String, String), EffectivePermissions>,
        HashMap<String, HashSet<(String, String)>>,
    ),
    DawnStoreError,
> {
    let mut permissions: HashMap<(String, String), EffectivePermissions> = HashMap::new();
    let mut resource_index: HashMap<String, HashSet<(String, String)>> = HashMap::new();

    // ── Index all roles ───────────────────────────────────────────────────────
    let role_objects = backend
        .get_objects(&BackendGetObjectsFilter {
            kind: Some(KIND_ROLE.to_string()),
            ..Default::default()
        })
        .await?;
    let mut roles: HashMap<String, Vec<GrantedScope>> = HashMap::new();
    for obj in &role_objects {
        let role: Role = serde_json::from_value(obj.spec.clone())?;
        roles.insert(
            object_string_id(&obj.namespace, &obj.kind, &obj.name),
            scopes_from_role(&role),
        );
    }

    // ── Index all global roles ────────────────────────────────────────────────
    let gr_objects = backend
        .get_objects(&BackendGetObjectsFilter {
            kind: Some(KIND_GLOBAL_ROLE.to_string()),
            ..Default::default()
        })
        .await?;
    let mut global_roles: HashMap<String, Vec<GrantedScope>> = HashMap::new();
    for obj in &gr_objects {
        let gr: GlobalRole = serde_json::from_value(obj.spec.clone())?;
        global_roles.insert(
            object_string_id(&obj.namespace, &obj.kind, &obj.name),
            scopes_from_global_role(&gr),
        );
    }

    // ── Process all role bindings ─────────────────────────────────────────────
    let rb_objects = backend
        .get_objects(&BackendGetObjectsFilter {
            kind: Some(KIND_ROLE_BINDING.to_string()),
            ..Default::default()
        })
        .await?;
    for obj in &rb_objects {
        let rb: RoleBinding = serde_json::from_value(obj.spec.clone())?;
        let rb_sid = object_string_id(&obj.namespace, &obj.kind, &obj.name);
        let role_sid = resolve_fk(&rb.role, &obj.namespace, KIND_ROLE);
        let Some(scopes) = roles.get(&role_sid) else { continue };
        let scopes = scopes.clone();
        for subject in &rb.subjects {
            let sa_sid = resolve_fk(subject, &obj.namespace, KIND_SERVICE_ACCOUNT);
            let parts: Vec<&str> = sa_sid.splitn(3, '/').collect();
            let (sa_ns, sa_name) = match parts.as_slice() {
                [ns, _, name] => (ns.to_string(), name.to_string()),
                _ => continue,
            };
            let sa_key = (sa_ns, sa_name);
            permissions.entry(sa_key.clone()).or_default().namespaced.extend(scopes.clone());
            resource_index.entry(rb_sid.clone()).or_default().insert(sa_key.clone());
            resource_index.entry(role_sid.clone()).or_default().insert(sa_key);
        }
    }

    // ── Process all global role bindings ──────────────────────────────────────
    let grb_objects = backend
        .get_objects(&BackendGetObjectsFilter {
            kind: Some(KIND_GLOBAL_ROLE_BINDING.to_string()),
            ..Default::default()
        })
        .await?;
    for obj in &grb_objects {
        let grb: GlobalRoleBinding = serde_json::from_value(obj.spec.clone())?;
        let grb_sid = object_string_id(&obj.namespace, &obj.kind, &obj.name);
        let role_sid = resolve_fk(&grb.role, &obj.namespace, KIND_GLOBAL_ROLE);
        let Some(scopes) = global_roles.get(&role_sid) else { continue };
        let scopes = scopes.clone();
        for subject in &grb.subjects {
            let sa_sid = resolve_fk(subject, &obj.namespace, KIND_SERVICE_ACCOUNT);
            let parts: Vec<&str> = sa_sid.splitn(3, '/').collect();
            let (sa_ns, sa_name) = match parts.as_slice() {
                [ns, _, name] => (ns.to_string(), name.to_string()),
                _ => continue,
            };
            let sa_key = (sa_ns, sa_name);
            permissions.entry(sa_key.clone()).or_default().global.extend(scopes.clone());
            resource_index.entry(grb_sid.clone()).or_default().insert(sa_key.clone());
            resource_index.entry(role_sid.clone()).or_default().insert(sa_key);
        }
    }

    Ok((permissions, resource_index))
}
