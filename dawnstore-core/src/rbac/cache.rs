use std::collections::{HashMap, HashSet};
use std::sync::RwLock;

use dawnstore_lib::GetObjectsFilter;

use crate::abstractions::DawnstoreBackend;
use crate::error::DawnStoreError;
use super::constants::{
    KIND_GLOBAL_ROLE, KIND_GLOBAL_ROLE_BINDING, KIND_ROLE, KIND_ROLE_BINDING,
    KIND_SERVICE_ACCOUNT,
};
use super::helpers::object_string_id;
use super::models::{GlobalRole, GlobalRoleBinding, PolicyRule, Role, RoleBinding};

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
    fn from_rule(rule: &PolicyRule) -> Self {
        let verbs = rule.verbs.iter().filter_map(|v| Verb::from_str(v)).collect();
        Self {
            api_version: rule.api_version.clone(),
            kinds: rule.kinds.clone(),
            verbs,
            names: rule.names.clone(),
        }
    }

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

// ── RbacCache ─────────────────────────────────────────────────────────────────

#[derive(Default)]
struct Inner {
    /// Maps `(namespace, sa_name)` → effective permission set.
    permissions: HashMap<(String, String), EffectivePermissions>,
    /// Maps a RBAC object string ID to the SA keys whose permissions it contributed to.
    /// Used to evict only the affected entries on apply/delete of an RBAC resource.
    resource_index: HashMap<String, HashSet<(String, String)>>,
}

/// Thread-safe in-memory RBAC permission cache.
///
/// Entries are populated eagerly via [`RbacCache::warm`] at startup and lazily
/// via a DB fallback on cache misses. Entries are evicted whenever a contributing
/// RBAC object (role, rolebinding, globalrole, globalrolebinding) is mutated.
#[derive(Default)]
pub struct RbacCache(RwLock<Inner>);

impl RbacCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Pre-populate the cache from all RBAC objects currently in the backend.
    /// Call this at startup after [`crate::rbac::init`].
    pub async fn warm<B: DawnstoreBackend>(&self, backend: &B) -> Result<(), DawnStoreError> {
        let (permissions, resource_index) = build_full_cache(backend).await?;
        let mut inner = self.0.write().unwrap();
        inner.permissions = permissions;
        inner.resource_index = resource_index;
        Ok(())
    }

    /// Return the effective permissions for `(namespace, sa_name)`.
    ///
    /// On a cache miss the backend is queried and the result is stored for
    /// future lookups.
    pub async fn get_or_load<B: DawnstoreBackend>(
        &self,
        backend: &B,
        namespace: &str,
        sa_name: &str,
    ) -> Result<EffectivePermissions, DawnStoreError> {
        let key = (namespace.to_string(), sa_name.to_string());

        // Fast path — read lock, no await.
        {
            let inner = self.0.read().unwrap();
            if let Some(perms) = inner.permissions.get(&key) {
                return Ok(perms.clone());
            }
        }

        // Cache miss — query the backend (no lock held across await).
        let (perms, resources) = load_sa_permissions(backend, namespace, sa_name).await?;

        // Write the result back.
        {
            let mut inner = self.0.write().unwrap();
            inner.permissions.insert(key.clone(), perms.clone());
            for res_id in resources {
                inner.resource_index.entry(res_id).or_default().insert(key.clone());
            }
        }

        Ok(perms)
    }

    /// Evict all cache entries that were derived from `object_string_id`.
    ///
    /// Call this after a successful `apply` or `delete` of a role, rolebinding,
    /// globalrole, or globalrolebinding object. The next `get_or_load` for any
    /// affected SA will re-query the backend.
    pub fn invalidate(&self, object_string_id: &str) {
        let mut inner = self.0.write().unwrap();
        if let Some(affected_keys) = inner.resource_index.remove(object_string_id) {
            for key in affected_keys {
                inner.permissions.remove(&key);
            }
        }
    }
}

// ── Private helpers ───────────────────────────────────────────────────────────

/// Resolve a FK string to a canonical `namespace/kind/name` string ID,
/// filling in missing segments from `default_ns` / `default_kind`.
fn resolve_fk(value: &str, default_ns: &str, default_kind: &str) -> String {
    let parts: Vec<&str> = value.splitn(3, '/').collect();
    match parts.as_slice() {
        [ns, kind, name] => object_string_id(ns, kind, name),
        [kind, name] => object_string_id(default_ns, kind, name),
        [name] => object_string_id(default_ns, default_kind, name),
        _ => value.to_string(),
    }
}

/// Extract just the `name` segment from a FK string (the last `/`-separated part).
fn name_from_fk(value: &str) -> &str {
    value.rsplit('/').next().unwrap_or(value)
}

fn scopes_from_role(role: &Role) -> Vec<GrantedScope> {
    role.rules.iter().map(GrantedScope::from_rule).collect()
}

fn scopes_from_global_role(role: &GlobalRole) -> Vec<GrantedScope> {
    role.rules.iter().map(GrantedScope::from_rule).collect()
}

/// Build the complete permission map by loading every RBAC object from the backend.
/// This is the bulk path used by [`RbacCache::warm`].
async fn build_full_cache<B: DawnstoreBackend>(
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
        .get(&GetObjectsFilter { kind: Some(KIND_ROLE.to_string()), ..Default::default() })
        .await?;

    let mut roles: HashMap<String, Vec<GrantedScope>> = HashMap::new();
    for obj in &role_objects {
        let role: Role = serde_json::from_value(obj.spec.clone())?;
        roles.insert(object_string_id(&obj.namespace, &obj.kind, &obj.name), scopes_from_role(&role));
    }

    // ── Index all global roles ────────────────────────────────────────────────
    let gr_objects = backend
        .get(&GetObjectsFilter { kind: Some(KIND_GLOBAL_ROLE.to_string()), ..Default::default() })
        .await?;

    let mut global_roles: HashMap<String, Vec<GrantedScope>> = HashMap::new();
    for obj in &gr_objects {
        let gr: GlobalRole = serde_json::from_value(obj.spec.clone())?;
        global_roles.insert(object_string_id(&obj.namespace, &obj.kind, &obj.name), scopes_from_global_role(&gr));
    }

    // ── Process all role bindings ─────────────────────────────────────────────
    let rb_objects = backend
        .get(&GetObjectsFilter { kind: Some(KIND_ROLE_BINDING.to_string()), ..Default::default() })
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
        .get(&GetObjectsFilter {
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

/// Load permissions for a single SA from the backend (used on cache miss).
/// Returns `(permissions, contributing_resource_string_ids)`.
async fn load_sa_permissions<B: DawnstoreBackend>(
    backend: &B,
    namespace: &str,
    sa_name: &str,
) -> Result<(EffectivePermissions, Vec<String>), DawnStoreError> {
    let mut perms = EffectivePermissions::default();
    let mut contributing: Vec<String> = Vec::new();
    let sa_full = object_string_id(namespace, KIND_SERVICE_ACCOUNT, sa_name);

    // ── Namespace-scoped: rolebindings in the SA's namespace ──────────────────
    let rbs = backend
        .get(&GetObjectsFilter {
            namespace: Some(namespace.to_string()),
            kind: Some(KIND_ROLE_BINDING.to_string()),
            ..Default::default()
        })
        .await?;

    for obj in &rbs {
        let rb: RoleBinding = serde_json::from_value(obj.spec.clone())?;

        let is_subject = rb
            .subjects
            .iter()
            .any(|s| resolve_fk(s, &obj.namespace, KIND_SERVICE_ACCOUNT) == sa_full);
        if !is_subject {
            continue;
        }

        let rb_sid = object_string_id(&obj.namespace, &obj.kind, &obj.name);
        let role_name = name_from_fk(&rb.role).to_string();

        let roles = backend
            .get(&GetObjectsFilter {
                namespace: Some(namespace.to_string()),
                kind: Some(KIND_ROLE.to_string()),
                name: Some(role_name),
                ..Default::default()
            })
            .await?;

        if let Some(role_obj) = roles.first() {
            let role: Role = serde_json::from_value(role_obj.spec.clone())?;
            let role_sid = object_string_id(&role_obj.namespace, &role_obj.kind, &role_obj.name);
            perms.namespaced.extend(scopes_from_role(&role));
            contributing.push(rb_sid);
            contributing.push(role_sid);
        }
    }

    // ── Global: all globalrolebindings ────────────────────────────────────────
    let grbs = backend
        .get(&GetObjectsFilter {
            kind: Some(KIND_GLOBAL_ROLE_BINDING.to_string()),
            ..Default::default()
        })
        .await?;

    for obj in &grbs {
        let grb: GlobalRoleBinding = serde_json::from_value(obj.spec.clone())?;

        let is_subject = grb
            .subjects
            .iter()
            .any(|s| resolve_fk(s, &obj.namespace, KIND_SERVICE_ACCOUNT) == sa_full);
        if !is_subject {
            continue;
        }

        let grb_sid = object_string_id(&obj.namespace, &obj.kind, &obj.name);
        let role_name = name_from_fk(&grb.role).to_string();

        let grs = backend
            .get(&GetObjectsFilter {
                kind: Some(KIND_GLOBAL_ROLE.to_string()),
                name: Some(role_name),
                ..Default::default()
            })
            .await?;

        if let Some(gr_obj) = grs.first() {
            let gr: GlobalRole = serde_json::from_value(gr_obj.spec.clone())?;
            let gr_sid = object_string_id(&gr_obj.namespace, &gr_obj.kind, &gr_obj.name);
            perms.global.extend(scopes_from_global_role(&gr));
            contributing.push(grb_sid);
            contributing.push(gr_sid);
        }
    }

    Ok((perms, contributing))
}
