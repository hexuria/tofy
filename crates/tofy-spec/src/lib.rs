//! Language-agnostic resource spec (IR) for tofy.
//!
//! Rust builders and other frontends emit this shape. The engine consumes JSON.

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
    Aws,
}

impl Backend {
    /// OpenTofu engine (docker provider or AWS provider). Default Local is Docker.
    pub fn uses_opentofu(self) -> bool {
        matches!(self, Backend::Tofu | Backend::Aws)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Postgres,
    Mysql,
    Redis,
    Bucket,
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Kind::Postgres => write!(f, "postgres"),
            Kind::Mysql => write!(f, "mysql"),
            Kind::Redis => write!(f, "redis"),
            Kind::Bucket => write!(f, "bucket"),
        }
    }
}

/// App-adjacent size. Local backend maps this to memory/CPU.
/// A later OpenTofu backend maps the same token to instance class.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Size {
    #[default]
    Small,
    Medium,
    Large,
}

impl Size {
    pub fn as_str(self) -> &'static str {
        match self {
            Size::Small => "small",
            Size::Medium => "medium",
            Size::Large => "large",
        }
    }

    /// Docker `--memory`.
    pub fn docker_memory(self) -> &'static str {
        match self {
            Size::Small => "256m",
            Size::Medium => "512m",
            Size::Large => "1g",
        }
    }

    /// Docker `--cpus`.
    pub fn docker_cpus(self) -> &'static str {
        match self {
            Size::Small => "0.25",
            Size::Medium => "0.50",
            Size::Large => "1.00",
        }
    }

    /// kreuzwerker/docker `memory` (MB).
    pub fn docker_memory_mb(self) -> u32 {
        match self {
            Size::Small => 256,
            Size::Medium => 512,
            Size::Large => 1024,
        }
    }

    /// Docker default swap ceiling when only memory is set: 2× memory (MB).
    /// Emitted so `tofu plan` is not permanently dirty (`memory_swap = 512 -> null`).
    pub fn docker_memory_swap_mb(self) -> u32 {
        self.docker_memory_mb().saturating_mul(2)
    }

    /// RDS `instance_class` for [`Backend::Aws`].
    pub fn aws_rds_instance_class(self) -> &'static str {
        match self {
            Size::Small => "db.t4g.micro",
            Size::Medium => "db.t4g.small",
            Size::Large => "db.t4g.medium",
        }
    }

    /// ElastiCache Redis `node_type` for [`Backend::Aws`].
    pub fn aws_elasticache_node_type(self) -> &'static str {
        match self {
            Size::Small => "cache.t4g.micro",
            Size::Medium => "cache.t4g.small",
            Size::Large => "cache.t4g.medium",
        }
    }

    /// S3 storage class for [`Backend::Aws`]. S3 has no instance class.
    pub fn aws_s3_storage_class(self) -> &'static str {
        match self {
            Size::Small | Size::Medium | Size::Large => "STANDARD",
        }
    }
}

impl fmt::Display for Size {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Who can reach the published host port. In-stack traffic uses the private network.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum Bind {
    #[default]
    #[serde(rename = "127.0.0.1")]
    Localhost,
    #[serde(rename = "0.0.0.0")]
    All,
}

impl Bind {
    pub fn as_ip(self) -> &'static str {
        match self {
            Bind::Localhost => "127.0.0.1",
            Bind::All => "0.0.0.0",
        }
    }
}

impl fmt::Display for Bind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_ip())
    }
}

impl Kind {
    pub fn default_port(self) -> u16 {
        match self {
            Kind::Postgres => 5432,
            Kind::Mysql => 3306,
            Kind::Redis => 6379,
            Kind::Bucket => 9000,
        }
    }

    pub fn default_version(self) -> &'static str {
        match self {
            Kind::Postgres => "16",
            Kind::Mysql => "8",
            Kind::Redis => "7",
            Kind::Bucket => "latest",
        }
    }

    pub fn internal_port(self) -> u16 {
        self.default_port()
    }
}

fn is_default_size(s: &Size) -> bool {
    *s == Size::Small
}

fn is_default_bind(b: &Bind) -> bool {
    *b == Bind::Localhost
}

fn default_replicas() -> u32 {
    1
}

fn is_one(n: &u32) -> bool {
    *n == 1
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
    #[serde(default, skip_serializing_if = "is_default_size")]
    pub size: Size,
    #[serde(default, skip_serializing_if = "is_default_bind")]
    pub bind: Bind,
    #[serde(default = "default_replicas", skip_serializing_if = "is_one")]
    pub replicas: u32,
}

impl Resource {
    pub fn new(name: impl Into<String>, kind: Kind) -> Self {
        Self {
            name: name.into(),
            kind,
            version: None,
            port: None,
            size: Size::Small,
            bind: Bind::Localhost,
            replicas: 1,
        }
    }

    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    pub fn with_port(mut self, port: u16) -> Self {
        self.port = Some(port);
        self
    }

    pub fn with_size(mut self, size: Size) -> Self {
        self.size = size;
        self
    }

    pub fn with_bind(mut self, bind: Bind) -> Self {
        self.bind = bind;
        self
    }

    pub fn with_replicas(mut self, replicas: u32) -> Self {
        self.replicas = replicas;
        self
    }

    pub fn version_or_default(&self) -> &str {
        self.version
            .as_deref()
            .unwrap_or_else(|| self.kind.default_version())
    }

    pub fn port_or_default(&self) -> u16 {
        self.port.unwrap_or_else(|| self.kind.default_port())
    }

    pub fn replicas_or_default(&self) -> u32 {
        self.replicas.max(1)
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
            if r.replicas == 0 {
                return Err(SpecError::Validation(format!(
                    "resource {} replicas must be >= 1",
                    r.name
                )));
            }
            if r.replicas > 1 {
                if r.kind == Kind::Bucket {
                    return Err(SpecError::Validation(
                        "bucket has no HA: replicas must be 1".into(),
                    ));
                }
                if self.backend == Backend::Aws {
                    return Err(SpecError::Validation(format!(
                        "aws backend has no HA: {} replicas must be 1",
                        r.kind
                    )));
                }
            }
        }
        Ok(())
    }

    /// Stack-private Docker network name: `tofy-{project}`.
    pub fn docker_network(&self) -> String {
        docker_network(&self.project)
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

pub fn docker_network(project: &str) -> String {
    format!("tofy-{project}")
}

/// Replica 0 uses the resource container name; later replicas append `-2`, `-3`, …
pub fn replica_container(project: &str, resource: &str, index: u32) -> String {
    if index == 0 {
        container_name(project, resource)
    } else {
        format!("{}-{}", container_name(project, resource), index + 1)
    }
}

/// In-stack DNS alias: the resource name for replica 0; `name-2`, `name-3`, … after that.
pub fn replica_alias(resource: &str, index: u32) -> String {
    if index == 0 {
        resource.to_string()
    } else {
        format!("{}-{}", resource, index + 1)
    }
}

pub fn replica_volume(project: &str, resource: &str, index: u32) -> String {
    if index == 0 {
        volume_name(project, resource)
    } else {
        format!("{}-{}", volume_name(project, resource), index + 1)
    }
}

/// In-stack DNS name: the resource name on the private network.
pub fn internal_host(resource: &str) -> &str {
    resource
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

    #[test]
    fn parse_size_bind_replicas() {
        let spec = Project::from_json_str(
            r#"{
                "project": "demo",
                "resources": [
                    {"name": "appdb", "type": "postgres", "size": "medium", "bind": "0.0.0.0"},
                    {"name": "cache", "type": "redis", "size": "large"}
                ]
            }"#,
        )
        .unwrap();
        assert_eq!(spec.resources[0].size, Size::Medium);
        assert_eq!(spec.resources[0].bind, Bind::All);
        assert_eq!(spec.resources[0].replicas, 1);
        assert_eq!(spec.resources[1].size, Size::Large);
        assert_eq!(spec.resources[1].replicas, 1);
        assert_eq!(spec.docker_network(), "tofy-demo");
        assert_eq!(internal_host("appdb"), "appdb");
    }

    #[test]
    fn defaults_omit_size_bind_replicas() {
        let spec = Project::from_json_str(demo_json()).unwrap();
        assert_eq!(spec.resources[0].size, Size::Small);
        assert_eq!(spec.resources[0].bind, Bind::Localhost);
        assert_eq!(spec.resources[0].replicas, 1);
        let json = spec.to_json_pretty().unwrap();
        assert!(!json.contains("\"size\""));
        assert!(!json.contains("\"replicas\""));
        assert!(!json.contains("0.0.0.0"));
    }

    #[test]
    fn docker_backends_allow_engine_replicas() {
        for backend in ["local", "tofu"] {
            for typ in ["postgres", "mysql", "redis"] {
                let spec = Project::from_json_str(&format!(
                    r#"{{"project":"demo","backend":"{backend}","resources":[{{"name":"x","type":"{typ}","replicas":2}}]}}"#
                ))
                .unwrap();
                assert_eq!(spec.resources[0].replicas, 2, "{backend} {typ}");
            }
        }
    }

    #[test]
    fn aws_backend_rejects_replicas() {
        for typ in ["postgres", "mysql", "redis", "bucket"] {
            let err = Project::from_json_str(&format!(
                r#"{{"project":"demo","backend":"aws","resources":[{{"name":"x","type":"{typ}","replicas":2}}]}}"#
            ))
            .unwrap_err();
            let msg = err.to_string();
            if typ == "bucket" {
                assert!(msg.contains("bucket has no HA"), "{typ}: {msg}");
            } else {
                assert!(msg.contains("aws backend has no HA"), "{typ}: {msg}");
            }
        }
    }

    #[test]
    fn bucket_rejects_replicas_on_docker_backends() {
        for backend in ["local", "tofu"] {
            let err = Project::from_json_str(&format!(
                r#"{{"project":"demo","backend":"{backend}","resources":[{{"name":"x","type":"bucket","replicas":2}}]}}"#
            ))
            .unwrap_err();
            assert!(
                err.to_string().contains("bucket has no HA"),
                "{backend}: {err}"
            );
        }
    }

    #[test]
    fn replica_alias_and_container_names() {
        assert_eq!(replica_alias("appdb", 0), "appdb");
        assert_eq!(replica_alias("appdb", 1), "appdb-2");
        assert_eq!(replica_container("demo", "appdb", 0), "tofy-demo-appdb");
        assert_eq!(replica_container("demo", "appdb", 1), "tofy-demo-appdb-2");
    }

    #[test]
    fn size_maps() {
        assert_eq!(Size::Small.docker_memory(), "256m");
        assert_eq!(Size::Small.docker_cpus(), "0.25");
        assert_eq!(Size::Small.docker_memory_mb(), 256);
        assert_eq!(Size::Small.docker_memory_swap_mb(), 512);
        assert_eq!(Size::Medium.docker_memory(), "512m");
        assert_eq!(Size::Medium.docker_memory_mb(), 512);
        assert_eq!(Size::Medium.docker_memory_swap_mb(), 1024);
        assert_eq!(Size::Large.docker_memory(), "1g");
        assert_eq!(Size::Large.docker_memory_mb(), 1024);
        assert_eq!(Size::Large.docker_memory_swap_mb(), 2048);
        assert_eq!(Size::Large.docker_cpus(), "1.00");
    }

    #[test]
    fn parse_tofu_backend() {
        let spec = Project::from_json_str(
            r#"{"project":"demo","backend":"tofu","resources":[{"name":"cache","type":"redis"}]}"#,
        )
        .unwrap();
        assert_eq!(spec.backend, Backend::Tofu);
        assert_eq!(spec.resources[0].replicas, 1);
        assert!(spec.backend.uses_opentofu());
    }

    #[test]
    fn parse_aws_backend() {
        let spec = Project::from_json_str(
            r#"{"project":"demoaws","backend":"aws","resources":[{"name":"uploads","type":"bucket"}]}"#,
        )
        .unwrap();
        assert_eq!(spec.backend, Backend::Aws);
        assert!(spec.backend.uses_opentofu());
        assert!(!Backend::Local.uses_opentofu());
    }

    #[test]
    fn size_maps_aws_classes() {
        assert_eq!(Size::Small.aws_rds_instance_class(), "db.t4g.micro");
        assert_eq!(Size::Medium.aws_rds_instance_class(), "db.t4g.small");
        assert_eq!(Size::Large.aws_rds_instance_class(), "db.t4g.medium");
        assert_eq!(Size::Small.aws_elasticache_node_type(), "cache.t4g.micro");
        assert_eq!(Size::Medium.aws_elasticache_node_type(), "cache.t4g.small");
        assert_eq!(Size::Large.aws_elasticache_node_type(), "cache.t4g.medium");
        assert_eq!(Size::Small.aws_s3_storage_class(), "STANDARD");
        assert_eq!(Size::Medium.aws_s3_storage_class(), "STANDARD");
        assert_eq!(Size::Large.aws_s3_storage_class(), "STANDARD");
    }
}
