//! Language-agnostic resource spec (IR) for tofy.
//!
//! Rust builders, YAML importers, and other frontends all emit this shape.
//! The engine consumes JSON.

use std::collections::BTreeSet;
use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum SpecError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Validation(String),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    #[default]
    Local,
    Tofu,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Postgres,
    Redis,
    Bucket,
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Kind::Postgres => write!(f, "postgres"),
            Kind::Redis => write!(f, "redis"),
            Kind::Bucket => write!(f, "bucket"),
        }
    }
}

impl Kind {
    pub fn default_port(self) -> u16 {
        match self {
            Kind::Postgres => 5432,
            Kind::Redis => 6379,
            Kind::Bucket => 9000,
        }
    }

    pub fn default_version(self) -> &'static str {
        match self {
            Kind::Postgres => "16",
            Kind::Redis => "7",
            Kind::Bucket => "latest",
        }
    }

    pub fn internal_port(self) -> u16 {
        self.default_port()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Resource {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: Kind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

impl Resource {
    pub fn new(name: impl Into<String>, kind: Kind) -> Self {
        Self {
            name: name.into(),
            kind,
            version: None,
            port: None,
        }
    }

    pub fn version_or_default(&self) -> &str {
        self.version
            .as_deref()
            .unwrap_or_else(|| self.kind.default_version())
    }

    pub fn port_or_default(&self) -> u16 {
        self.port.unwrap_or_else(|| self.kind.default_port())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Project {
    pub project: String,
    #[serde(default)]
    pub backend: Backend,
    #[serde(default)]
    pub resources: Vec<Resource>,
}

impl Project {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            project: name.into(),
            backend: Backend::Local,
            resources: Vec::new(),
        }
    }

    pub fn from_json_str(raw: &str) -> Result<Self, SpecError> {
        let spec: Self = serde_json::from_str(raw)?;
        spec.validate()?;
        Ok(spec)
    }

    pub fn load_json(path: &Path) -> Result<Self, SpecError> {
        Self::from_json_str(&std::fs::read_to_string(path)?)
    }

    pub fn to_json_pretty(&self) -> Result<String, SpecError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    pub fn validate(&self) -> Result<(), SpecError> {
        if self.project.trim().is_empty() {
            return Err(SpecError::Validation("project name is empty".into()));
        }
        let mut names = BTreeSet::new();
        for r in &self.resources {
            if r.name.trim().is_empty() {
                return Err(SpecError::Validation("resource name is empty".into()));
            }
            if !is_ident(&r.name) {
                return Err(SpecError::Validation(format!(
                    "resource name {:?} must be [A-Za-z][A-Za-z0-9_-]*",
                    r.name
                )));
            }
            if !names.insert(r.name.clone()) {
                return Err(SpecError::Validation(format!(
                    "duplicate resource {}",
                    r.name
                )));
            }
        }
        Ok(())
    }

    pub fn resource(&self, name: &str) -> Option<&Resource> {
        self.resources.iter().find(|r| r.name == name)
    }
}

fn is_ident(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// `TOFY_<RESOURCE>_<KEY>`, e.g. `TOFY_APPDB_URI`.
pub fn env_var(resource: &str, key: &str) -> String {
    format!("TOFY_{}_{}", env_token(resource), env_token(key))
}

pub fn env_token(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

pub fn is_secret_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    let k = k.strip_prefix("tofy_").unwrap_or(&k);
    k == "password"
        || k == "secret"
        || k == "secret_key"
        || k == "access_key"
        || k == "uri"
        || k.ends_with("_password")
        || k.ends_with("_secret")
        || k.ends_with("_secret_key")
        || k.ends_with("_access_key")
        || k.ends_with("_uri")
}

pub fn container_name(project: &str, resource: &str) -> String {
    format!("tofy-{project}-{resource}")
}

pub fn volume_name(project: &str, resource: &str) -> String {
    format!("tofy-{project}-{resource}-data")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo_json() -> &'static str {
        r#"{
            "project": "demo",
            "backend": "local",
            "resources": [
                {"name": "appdb", "type": "postgres", "version": "16", "port": 5433},
                {"name": "cache", "type": "redis"},
                {"name": "uploads", "type": "bucket"}
            ]
        }"#
    }

    #[test]
    fn parse_project_json() {
        let spec = Project::from_json_str(demo_json()).unwrap();
        assert_eq!(spec.project, "demo");
        assert_eq!(spec.backend, Backend::Local);
        assert_eq!(spec.resources.len(), 3);
        assert_eq!(spec.resources[0].kind, Kind::Postgres);
        assert_eq!(spec.resources[0].version.as_deref(), Some("16"));
        assert_eq!(spec.resources[0].port, Some(5433));
        assert_eq!(spec.resources[1].kind, Kind::Redis);
        assert_eq!(spec.resources[1].port_or_default(), 6379);
        assert_eq!(spec.resources[2].kind, Kind::Bucket);
        assert_eq!(spec.resources[2].port_or_default(), 9000);
    }

    #[test]
    fn json_roundtrip() {
        let spec = Project::from_json_str(demo_json()).unwrap();
        let again = Project::from_json_str(&spec.to_json_pretty().unwrap()).unwrap();
        assert_eq!(spec, again);
    }

    #[test]
    fn reject_duplicate_names() {
        let err = Project::from_json_str(
            r#"{
                "project": "demo",
                "resources": [
                    {"name": "db", "type": "postgres"},
                    {"name": "db", "type": "redis"}
                ]
            }"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("duplicate"));
    }

    #[test]
    fn reject_empty_project() {
        let err = Project::from_json_str(r#"{"project":"","resources":[]}"#).unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn env_var_naming() {
        assert_eq!(env_var("appdb", "uri"), "TOFY_APPDB_URI");
        assert_eq!(env_var("appdb", "password"), "TOFY_APPDB_PASSWORD");
        assert_eq!(env_var("cache", "uri"), "TOFY_CACHE_URI");
        assert_eq!(env_var("uploads", "endpoint"), "TOFY_UPLOADS_ENDPOINT");
        assert_eq!(env_var("my-db", "secret_key"), "TOFY_MY_DB_SECRET_KEY");
    }

    #[test]
    fn secret_key_classification() {
        assert!(is_secret_key("password"));
        assert!(is_secret_key("secret_key"));
        assert!(is_secret_key("access_key"));
        assert!(is_secret_key("uri"));
        assert!(is_secret_key("TOFY_APPDB_PASSWORD"));
        assert!(is_secret_key("TOFY_APPDB_URI"));
        assert!(is_secret_key("TOFY_UPLOADS_SECRET_KEY"));
        assert!(!is_secret_key("port"));
        assert!(!is_secret_key("TOFY_APPDB_PORT"));
        assert!(!is_secret_key("TOFY_APPDB_USER"));
        assert!(!is_secret_key("TOFY_UPLOADS_ENDPOINT"));
        assert!(!is_secret_key("TOFY_UPLOADS_BUCKET"));
    }
}
