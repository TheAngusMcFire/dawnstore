// ── API version ───────────────────────────────────────────────────────────────

pub const API_VERSION_V1: &str = "v1";

// ── Namespaces ────────────────────────────────────────────────────────────────

pub const SYSTEM_NAMESPACE: &str = "system";

// ── Kind names ────────────────────────────────────────────────────────────────

pub const KIND_NAMESPACE: &str = "namespace";
pub const KIND_SERVICE_ACCOUNT: &str = "serviceaccount";
pub const KIND_SERVICE_ACCOUNT_TOKEN: &str = "serviceaccounttoken";
pub const KIND_ROLE: &str = "role";
pub const KIND_GLOBAL_ROLE: &str = "globalrole";
pub const KIND_ROLE_BINDING: &str = "rolebinding";
pub const KIND_GLOBAL_ROLE_BINDING: &str = "globalrolebinding";

// ── Well-known object names ───────────────────────────────────────────────────

pub const SA_SUPERADMIN: &str = "superadmin";
pub const TOKEN_BOOTSTRAP: &str = "bootstrap";
