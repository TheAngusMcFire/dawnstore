use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use tokio::sync::RwLock as TokioRwLock;

use crate::abstractions::{BackendGetObjectsFilter, DawnstoreBackend, RawForeignKeyConstraint};
use crate::error::DawnStoreError;
use crate::rbac::constants::{
    KIND_GLOBAL_ROLE, KIND_GLOBAL_ROLE_BINDING, KIND_ROLE, KIND_ROLE_BINDING, KIND_SERVICE_ACCOUNT,
    SA_SUPERADMIN, SYSTEM_NAMESPACE,
};
use crate::rbac::helpers::{object_string_id, schema_cache_key};
use crate::rbac::middleware::Claims;
use crate::rbac::models::{GlobalRole, GlobalRoleBinding, Role, RoleBinding};

// ── Verb ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Verb {
    Get,
    Apply,
    Delete,
}

impl Verb {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "get" => Some(Verb::Get),
            "apply" => Some(Verb::Apply),
            "delete" => Some(Verb::Delete),
            _ => None,
        }
    }
}

// ── GrantedScope ──────────────────────────────────────────────────────────────

/// One collapsed permission grant derived from a `PolicyRule`.
#[derive(Debug, Clone)]
pub struct GrantedScope {
    /// `"*"` matches all api versions.
    pub api_version: String,
    /// `["*"]` matches all kinds.
    pub kinds: Vec<String>,
    pub verbs: HashSet<Verb>,
    /// `None` = all object names are permitted.
    pub names: Option<Vec<String>>,
}

impl GrantedScope {
    /// `true` if this scope grants `verb` on `(api_version, kind, name)`.
    pub fn matches(&self, verb: Verb, api_version: &str, kind: &str, name: &str) -> bool {
        self.verbs.contains(&verb)
            && (self.api_version == "*" || self.api_version == api_version)
            && self.kinds.iter().any(|k| k == "*" || k == kind)
            && self.names.as_ref().map_or(true, |ns| ns.iter().any(|n| n == name))
    }

    /// `true` if this scope grants `verb` on `(api_version, kind)` for any name.
    pub fn matches_kind(&self, verb: Verb, api_version: &str, kind: &str) -> bool {
        self.verbs.contains(&verb)
            && (self.api_version == "*" || self.api_version == api_version)
            && self.kinds.iter().any(|k| k == "*" || k == kind)
    }
}

// ── EffectivePermissions ──────────────────────────────────────────────────────

/// The collapsed union of all role rules bound to a service account.
#[derive(Debug, Clone, Default)]
pub struct EffectivePermissions {
    /// Grants from namespace-scoped `RoleBinding`s in the SA's own namespace.
    pub namespaced: Vec<GrantedScope>,
    /// Grants from `GlobalRoleBinding`s — apply regardless of namespace.
    pub global: Vec<GrantedScope>,
}

impl EffectivePermissions {
    /// Returns `true` if any grant allows `verb` on `(api_version, kind, name)`.
    pub fn is_allowed(&self, verb: Verb, api_version: &str, kind: &str, name: &str) -> bool {
        self.namespaced.iter().any(|s| s.matches(verb, api_version, kind, name))
            || self.global.iter().any(|s| s.matches(verb, api_version, kind, name))
    }
}

// ── is_superadmin ─────────────────────────────────────────────────────────────

/// Returns `true` if the caller is the system superadmin.
///
/// The superadmin (`system/serviceaccount/superadmin`) bypasses all
/// authorization checks and is the only identity that may issue tokens.
pub fn is_superadmin(claims: &Claims) -> bool {
    claims.namespace == SYSTEM_NAMESPACE && claims.sub == SA_SUPERADMIN
}

// ── Internal permission state ─────────────────────────────────────────────────

#[derive(Default)]
struct PermissionState {
    /// Maps `(namespace, sa_name)` → effective permission set.
    permissions: HashMap<(String, String), EffectivePermissions>,
    /// Maps an RBAC object string ID to the SA keys whose permissions it contributed to.
    resource_index: HashMap<String, HashSet<(String, String)>>,
}

// ── DawnstoreCache ────────────────────────────────────────────────────────────

/// Unified cache for schema validators, foreign key constraints, RBAC permissions,
/// and kind-alias resolution.
#[derive(Default)]
pub struct DawnstoreCache {
    schema: TokioRwLock<HashMap<String, Arc<jsonschema::Validator>>>,
    foreign_key: TokioRwLock<HashMap<String, Arc<Vec<RawForeignKeyConstraint>>>>,
    permission: RwLock<PermissionState>,
    /// Maps every registered alias (and canonical kind name) → canonical kind name.
    kind_alias: TokioRwLock<HashMap<String, String>>,
}

impl DawnstoreCache {
    // ── Init ──────────────────────────────────────────────────────────────────

    /// Initialise all three caches from `backend` in sequence.
    pub async fn init<B: DawnstoreBackend>(backend: &B) -> Result<Self, DawnStoreError> {
        let cache = Self::default();
        cache.init_schema(backend).await?;
        cache.init_foreign_key(backend).await?;
        cache.init_permission(backend).await?;
        Ok(cache)
    }

    /// Populate the schema cache from all schemas returned by `backend`.
    /// Also populates the kind-alias map so that alias resolution is available
    /// immediately after schema initialisation.
    pub async fn init_schema<B: DawnstoreBackend>(
        &self,
        backend: &B,
    ) -> Result<(), DawnStoreError> {
        let schemas = backend.load_all_schemas().await?;
        let mut cache = self.schema.write().await;
        let mut aliases = self.kind_alias.write().await;
        for schema in schemas {
            let key = schema_cache_key(&schema.api_version, &schema.kind);
            let value: serde_json::Value = serde_json::from_str(&schema.json_schema)?;
            let validator = Arc::new(jsonschema::validator_for(&value)?);
            cache.insert(key, validator);
            // Register the canonical kind and all its aliases.
            aliases.insert(schema.kind.clone(), schema.kind.clone());
            for alias in &schema.aliases {
                aliases.insert(alias.clone(), schema.kind.clone());
            }
        }
        Ok(())
    }

    /// Populate the FK cache from all constraints returned by `backend`.
    pub async fn init_foreign_key<B: DawnstoreBackend>(
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
    pub async fn init_permission<B: DawnstoreBackend>(
        &self,
        backend: &B,
    ) -> Result<(), DawnStoreError> {
        let (permissions, resource_index) = build_full_permission_cache(backend).await?;
        let mut state = self.permission.write().unwrap();
        state.permissions = permissions;
        state.resource_index = resource_index;
        Ok(())
    }

    // ── Kind-alias resolution ─────────────────────────────────────────────────

    /// Resolve `kind_or_alias` to the canonical kind name.
    /// Returns `Some(canonical_kind)` if the alias (or exact kind) is registered,
    /// or `None` if it has never been seen.
    pub async fn resolve_kind(&self, kind_or_alias: &str) -> Option<String> {
        self.kind_alias.read().await.get(kind_or_alias).cloned()
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
async fn build_full_permission_cache<B: DawnstoreBackend>(
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
