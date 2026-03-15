use dawnstore_lib::ReturnObject;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ── Namespace ─────────────────────────────────────────────────────────────────

/// A namespace is a grouping of objects. The spec is intentionally empty;
/// the name in the object metadata is the namespace identifier.
///
/// Namespace objects are stored in the `system` namespace.
/// The `system` namespace itself is seeded during [`crate::rbac::init`].
#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
#[schemars(deny_unknown_fields)]
pub struct Namespace {}

// ── PolicyRule ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
#[schemars(deny_unknown_fields)]
pub struct PolicyRule {
    /// `"*"` matches all api versions.
    pub api_version: String,
    /// `["*"]` matches all kinds.
    pub kinds: Vec<String>,
    /// Subset of `"get"`, `"apply"`, `"delete"`.
    pub verbs: Vec<String>,
    /// `None` = all object names; `Some([...])` = restrict to specific names.
    pub names: Option<Vec<String>>,
}

// ── ServiceAccount ────────────────────────────────────────────────────────────

/// An identity (service or user). The dawnstore object metadata provides
/// `name` and `namespace`; this spec is intentionally empty for now.
#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
#[schemars(deny_unknown_fields)]
pub struct ServiceAccount {}

// ── ServiceAccountToken ───────────────────────────────────────────────────────

/// A named credential bound to a `ServiceAccount`.
/// The JWT is derived from this object's UUID and is never stored.
#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
#[schemars(deny_unknown_fields)]
pub struct ServiceAccountToken {
    /// FK → ServiceAccount (same namespace), value is `namespace/service-account/name`.
    pub service_account: String,
    /// `None` = never expires.
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Navigation property: the referenced `ServiceAccount` object.
    /// When set on apply input, the embedded object is extracted and applied first.
    /// Populated on get when `fill_child_foreign_keys` is enabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub service_account_object: Option<ReturnObject<Box<ServiceAccount>>>,
}

// ── Role ──────────────────────────────────────────────────────────────────────

/// A namespace-scoped set of permissions.
#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
#[schemars(deny_unknown_fields)]
pub struct Role {
    pub rules: Vec<PolicyRule>,
}

// ── GlobalRole ────────────────────────────────────────────────────────────────

/// A cluster-scoped set of permissions (applies across all namespaces).
#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
#[schemars(deny_unknown_fields)]
pub struct GlobalRole {
    pub rules: Vec<PolicyRule>,
}

// ── RoleBinding ───────────────────────────────────────────────────────────────

/// Binds a `Role` to one or more `ServiceAccount`s within a namespace.
#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
#[schemars(deny_unknown_fields)]
pub struct RoleBinding {
    /// FK → Role (same namespace).
    pub role: String,
    /// FK → ServiceAccount, each value is `namespace/service-account/name`.
    pub subjects: Vec<String>,
    /// Navigation property: the bound `Role` object.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub role_object: Option<ReturnObject<Box<Role>>>,
    /// Navigation property: the bound `ServiceAccount` objects.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub subjects_objects: Option<Vec<ReturnObject<Box<ServiceAccount>>>>,
}

// ── GlobalRoleBinding ─────────────────────────────────────────────────────────

/// Binds a `GlobalRole` to one or more `ServiceAccount`s across all namespaces.
#[derive(Debug, Clone, JsonSchema, Serialize, Deserialize)]
#[schemars(deny_unknown_fields)]
pub struct GlobalRoleBinding {
    /// FK → GlobalRole.
    pub role: String,
    /// FK → ServiceAccount, each value is `namespace/service-account/name`.
    pub subjects: Vec<String>,
    /// Navigation property: the bound `GlobalRole` object.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub role_object: Option<ReturnObject<Box<GlobalRole>>>,
    /// Navigation property: the bound `ServiceAccount` objects.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub subjects_objects: Option<Vec<ReturnObject<Box<ServiceAccount>>>>,
}
