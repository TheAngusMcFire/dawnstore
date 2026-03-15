#![allow(dead_code)] // structs/enums are used as generic type params, not constructed directly

use dawnstore_lib::ReturnObject;

// ── Existing example model ────────────────────────────────────────────────────

#[derive(schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
#[schemars(deny_unknown_fields)]
pub struct Container {
    pub nr: u32,
    pub notes: Option<String>,
    pub parent: Option<String>,
    pub parent_object: Option<ReturnObject<Box<Container>>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(default)]
    pub items: Vec<String>,
}

// ── Platform domain models ────────────────────────────────────────────────────
//
// A simple software-platform domain that demonstrates dawnstore's key features:
//
//   Team ← Project ← Environment ← Secret
//                  ↘               ↗       ↖
//                    Deployment ───  (also references secrets: NoneOrMany)
//
// Features covered:
//   ForeignKeyType::One         — Project.team, Environment.project,
//                                 Deployment.project, Deployment.environment,
//                                 Secret.environment
//   ForeignKeyType::NoneOrMany  — Deployment.secrets (zero-or-more secrets)
//   Nav-props (*_object fields) — filled on GET when fill_child_foreign_keys=true
//   Enums                       — ProjectStatus, EnvType (JSON-schema validated)
//   Optional fields             — description, tags
//   Arrays (non-FK)             — Team.tags

/// A group of engineers responsible for one or more projects.
#[derive(schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
#[schemars(deny_unknown_fields)]
pub struct Team {
    pub description: Option<String>,
    /// Free-form labels, e.g. ["backend", "on-call"].
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Lifecycle state of a project.
#[derive(schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
pub enum ProjectStatus {
    Active,
    Archived,
}

/// A software project owned by exactly one team.
///
/// FK: `team` → Team (One, required)
#[derive(schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
#[schemars(deny_unknown_fields)]
pub struct Project {
    pub description: Option<String>,
    pub status: ProjectStatus,
    /// FK reference: `name`, `team/name`, or `namespace/team/name`.
    pub team: String,
    /// Navigation property — populated on GET when fill_child_foreign_keys=true.
    pub team_object: Option<ReturnObject<Box<Team>>>,
}

/// The tier of a deployment environment.
#[derive(schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
pub enum EnvType {
    Development,
    Staging,
    Production,
}

/// A named environment (dev / staging / prod) belonging to a project.
///
/// FK: `project` → Project (One, required)
#[derive(schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
#[schemars(deny_unknown_fields)]
pub struct Environment {
    pub env_type: EnvType,
    /// FK reference to the owning project.
    pub project: String,
    /// Navigation property — populated on GET when fill_child_foreign_keys=true.
    pub project_object: Option<ReturnObject<Box<Project>>>,
}

/// A named secret slot tied to an environment.
///
/// Only the key name (e.g. "DATABASE_URL") is stored — the actual value lives
/// in an external secret manager. This record exists so dawnstore can track
/// which secrets belong to each environment and enforce access control on them.
///
/// FK: `environment` → Environment (One, required)
#[derive(schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
#[schemars(deny_unknown_fields)]
pub struct Secret {
    /// The environment variable / config key name, e.g. "DATABASE_URL".
    pub key: String,
    pub description: Option<String>,
    /// FK reference to the owning environment.
    pub environment: String,
    /// Navigation property — populated on GET when fill_child_foreign_keys=true.
    pub environment_object: Option<ReturnObject<Box<Environment>>>,
}

/// A versioned deployment of a project into a specific environment.
///
/// FKs:
///   `project`     → Project     (One, required)
///   `environment` → Environment (One, required)
///   `secrets`     → Secret      (NoneOrMany — declares which secrets this
///                                deployment requires at runtime)
#[derive(schemars::JsonSchema, serde::Serialize, serde::Deserialize)]
#[schemars(deny_unknown_fields)]
pub struct Deployment {
    /// Container image reference, e.g. "registry.example.com/api:1.2.3".
    pub image: String,
    /// Semantic version string, e.g. "1.2.3".
    pub version: String,
    /// FK reference to the project being deployed.
    pub project: String,
    /// Navigation property — populated on GET when fill_child_foreign_keys=true.
    pub project_object: Option<ReturnObject<Box<Project>>>,
    /// FK reference to the target environment.
    pub environment: String,
    /// Navigation property — populated on GET when fill_child_foreign_keys=true.
    pub environment_object: Option<ReturnObject<Box<Environment>>>,
    /// FK references to the secrets this deployment requires (zero or more).
    /// Each entry is a Secret reference: `name`, `secret/name`, or `ns/secret/name`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(default)]
    pub secrets: Vec<String>,
    /// Navigation property — populated on GET when fill_child_foreign_keys=true.
    pub secrets_objects: Option<Vec<ReturnObject<Box<Secret>>>>,
}
