//! Constrained Docker Compose subset → JSON IR.
//!
//! This is an importer, not a yaml write path and not auto-load. Unknown
//! images fail. Secrets in Compose env are not copied into the spec.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;
use tofy_spec::{Backend, Bind, Kind, Project, Resource, Size};

use crate::error::{Error, Result};

#[derive(Debug, Deserialize)]
struct ComposeFile {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    services: BTreeMap<String, ComposeService>,
}

#[derive(Debug, Deserialize, Default)]
struct ComposeService {
    #[serde(default)]
    image: Option<String>,
    #[serde(default)]
    ports: Vec<ComposePort>,
    #[serde(default)]
    mem_limit: Option<serde_yaml::Value>,
    #[serde(default)]
    deploy: Option<ComposeDeploy>,
}

#[derive(Debug, Deserialize, Default)]
struct ComposeDeploy {
    #[serde(default)]
    replicas: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ComposePort {
    Short(String),
    Long {
        #[serde(default)]
        published: Option<serde_yaml::Value>,
        #[serde(default)]
        host_ip: Option<String>,
    },
}

/// Parse a Compose file into a validated [`Project`]. Does not apply.
pub fn from_compose_file(path: &Path, project: Option<&str>, backend: Backend) -> Result<Project> {
    let raw = std::fs::read_to_string(path)?;
    from_compose_str(&raw, project, backend, Some(path))
}

/// Parse Compose YAML text into a validated [`Project`]. Does not apply.
pub fn from_compose_str(
    yaml: &str,
    project: Option<&str>,
    backend: Backend,
    source: Option<&Path>,
) -> Result<Project> {
    let file: ComposeFile = serde_yaml::from_str(yaml)
        .map_err(|e| Error::Usage(format!("invalid compose yaml: {e}")))?;
    if file.services.is_empty() {
        return Err(Error::Usage("compose has no services".into()));
    }
    let project_name = resolve_project(project, file.name.as_deref(), source)?;
    let mut spec = Project::new(project_name);
    spec.backend = backend;
    for (name, svc) in file.services {
        spec.resources.push(service_to_resource(&name, &svc)?);
    }
    spec.validate()?;
    Ok(spec)
}

fn resolve_project(
    explicit: Option<&str>,
    compose_name: Option<&str>,
    source: Option<&Path>,
) -> Result<String> {
    if let Some(p) = explicit.map(str::trim).filter(|s| !s.is_empty()) {
        return Ok(p.to_string());
    }
    if let Some(p) = compose_name.map(str::trim).filter(|s| !s.is_empty()) {
        return Ok(p.to_string());
    }
    if let Some(dir) = source.and_then(|p| p.parent()).and_then(|p| p.file_name()) {
        let name = dir.to_string_lossy();
        if is_simple_ident(&name) {
            return Ok(name.into_owned());
        }
    }
    Err(Error::Usage(
        "compose import needs --project or a top-level name:".into(),
    ))
}

fn is_simple_ident(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn service_to_resource(name: &str, svc: &ComposeService) -> Result<Resource> {
    let image = svc
        .image
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Error::Usage(format!("service {name} has no image")))?;
    let (kind, tag) = kind_from_image(image)?;
    let mut r = Resource::new(name, kind);
    if let Some(tag) = tag {
        r.version = Some(tag);
    }
    if let Some((bind, port)) = first_port(&svc.ports)? {
        r.bind = bind;
        r.port = Some(port);
    }
    r.size = size_from_mem(svc.mem_limit.as_ref());
    if let Some(n) = svc.deploy.as_ref().and_then(|d| d.replicas) {
        r.replicas = n;
    }
    Ok(r)
}

/// Map a Docker image reference to a tofy kind. Unknown images fail.
pub fn kind_from_image(image: &str) -> Result<(Kind, Option<String>)> {
    let image = image.trim();
    let image = image.split('@').next().unwrap_or(image);
    let (name, tag) = split_name_tag(image);
    let kind = kind_from_name(name).ok_or_else(|| {
        Error::Usage(format!(
            "unknown compose image {image:?}; importer maps postgres, redis, mysql, and minio/minio only"
        ))
    })?;
    Ok((kind, tag.map(|s| s.to_string())))
}

fn split_name_tag(image: &str) -> (&str, Option<&str>) {
    match image.rsplit_once(':') {
        Some((name, tag)) if !tag.contains('/') => (name, Some(tag)),
        _ => (image, None),
    }
}

fn kind_from_name(name: &str) -> Option<Kind> {
    let name = name
        .strip_prefix("docker.io/")
        .unwrap_or(name)
        .strip_prefix("library/")
        .unwrap_or(name);
    if name == "minio/minio" || name.ends_with("/minio/minio") {
        return Some(Kind::Bucket);
    }
    let base = name.rsplit('/').next().unwrap_or(name);
    match base {
        "postgres" => Some(Kind::Postgres),
        "mysql" => Some(Kind::Mysql),
        "redis" => Some(Kind::Redis),
        _ => None,
    }
}

fn first_port(ports: &[ComposePort]) -> Result<Option<(Bind, u16)>> {
    if ports.is_empty() {
        return Ok(None);
    }
    match &ports[0] {
        ComposePort::Short(s) => parse_short_port(s).map(Some),
        ComposePort::Long { published, host_ip } => {
            let Some(published) = published else {
                return Ok(None);
            };
            let port = yaml_u16(published)?;
            let bind = bind_from_ip(host_ip.as_deref().unwrap_or(""));
            Ok(Some((bind, port)))
        }
    }
}

fn parse_short_port(raw: &str) -> Result<(Bind, u16)> {
    let s = raw.trim();
    let s = s.split('/').next().unwrap_or(s);
    let parts: Vec<&str> = s.split(':').collect();
    match parts.as_slice() {
        [host, published, _target] => {
            let port = parse_u16(published, raw)?;
            Ok((bind_from_ip(host), port))
        }
        [published, _target] => {
            let port = parse_u16(published, raw)?;
            Ok((Bind::All, port))
        }
        [published] => {
            let port = parse_u16(published, raw)?;
            Ok((Bind::All, port))
        }
        _ => Err(Error::Usage(format!("unsupported compose port {raw:?}"))),
    }
}

fn bind_from_ip(host: &str) -> Bind {
    if host.is_empty() || host == "0.0.0.0" || host == "::" {
        Bind::All
    } else if host == "127.0.0.1" || host == "localhost" {
        Bind::Localhost
    } else {
        Bind::All
    }
}

fn parse_u16(s: &str, raw: &str) -> Result<u16> {
    s.parse()
        .map_err(|_| Error::Usage(format!("unsupported compose port {raw:?}")))
}

fn yaml_u16(v: &serde_yaml::Value) -> Result<u16> {
    match v {
        serde_yaml::Value::Number(n) => n
            .as_u64()
            .and_then(|n| u16::try_from(n).ok())
            .ok_or_else(|| Error::Usage(format!("unsupported compose port {v:?}"))),
        serde_yaml::Value::String(s) => s
            .parse()
            .map_err(|_| Error::Usage(format!("unsupported compose port {s:?}"))),
        _ => Err(Error::Usage(format!("unsupported compose port {v:?}"))),
    }
}

fn size_from_mem(v: Option<&serde_yaml::Value>) -> Size {
    let Some(v) = v else {
        return Size::Small;
    };
    let s = match v {
        serde_yaml::Value::String(s) => s.to_ascii_lowercase().replace(' ', ""),
        _ => return Size::Small,
    };
    match s.as_str() {
        "256m" | "256mb" | "256mi" => Size::Small,
        "512m" | "512mb" | "512mi" => Size::Medium,
        "1g" | "1gb" | "1gi" | "1024m" | "1024mb" => Size::Large,
        _ => Size::Small,
    }
}

/// Write JSON IR. Never writes `docker-compose.yml`. Does not apply.
pub fn write_spec_json(spec: &Project, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(path, spec.to_json_pretty()?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEMO: &str = r#"
name: demo
services:
  appdb:
    image: postgres:16
    mem_limit: 256m
    ports:
      - "127.0.0.1:5433:5432"
  cache:
    image: redis:7
    mem_limit: 256m
    ports:
      - "127.0.0.1:6379:6379"
  uploads:
    image: minio/minio:latest
    mem_limit: 256m
    ports:
      - "127.0.0.1:9000:9000"
"#;

    #[test]
    fn three_service_compose_maps_kinds_ports_bind() {
        let spec = from_compose_str(DEMO, None, Backend::Local, None).unwrap();
        assert_eq!(spec.project, "demo");
        assert_eq!(spec.backend, Backend::Local);
        assert_eq!(spec.resources.len(), 3);
        let appdb = spec.resource("appdb").unwrap();
        assert_eq!(appdb.kind, Kind::Postgres);
        assert_eq!(appdb.port, Some(5433));
        assert_eq!(appdb.bind, Bind::Localhost);
        assert_eq!(appdb.version.as_deref(), Some("16"));
        let cache = spec.resource("cache").unwrap();
        assert_eq!(cache.kind, Kind::Redis);
        assert_eq!(cache.port, Some(6379));
        let uploads = spec.resource("uploads").unwrap();
        assert_eq!(uploads.kind, Kind::Bucket);
        assert_eq!(uploads.port, Some(9000));
        let json = spec.to_json_pretty().unwrap();
        assert!(!json.to_ascii_lowercase().contains("password"));
        assert!(!json.contains("POSTGRES_"));
    }

    #[test]
    fn unknown_image_errors() {
        let yaml = r#"
name: demo
services:
  web:
    image: nginx:latest
"#;
        let err = from_compose_str(yaml, None, Backend::Local, None).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("nginx"), "{msg}");
        assert!(msg.contains("unknown compose image"), "{msg}");
    }

    #[test]
    fn unknown_image_does_not_write_file() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("docker-compose.yml");
        let out = dir.path().join("spec.json");
        std::fs::write(
            &src,
            r#"
name: demo
services:
  web:
    image: nginx:latest
"#,
        )
        .unwrap();
        let err = from_compose_file(&src, None, Backend::Local).unwrap_err();
        assert!(err.to_string().contains("nginx"));
        assert!(!out.exists(), "must not write spec on failure");
        assert!(!dir.path().join(".tofy").join("docker-compose.yml").exists());
    }

    #[test]
    fn write_spec_json_is_not_compose_and_does_not_apply() {
        let spec = from_compose_str(DEMO, None, Backend::Local, None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("spec.json");
        write_spec_json(&spec, &out).unwrap();
        assert!(out.exists());
        assert!(!dir.path().join("docker-compose.yml").exists());
        assert!(!dir.path().join(".tofy").join("docker-compose.yml").exists());
        let loaded = Project::load_json(&out).unwrap();
        assert_eq!(loaded.project, "demo");
        assert_eq!(loaded.resources.len(), 3);
    }

    #[test]
    fn bind_all_from_unspecified_host() {
        let yaml = r#"
name: demo
services:
  cache:
    image: redis:7
    ports:
      - "6379:6379"
"#;
        let spec = from_compose_str(yaml, None, Backend::Local, None).unwrap();
        assert_eq!(spec.resource("cache").unwrap().bind, Bind::All);
    }

    #[test]
    fn replicas_over_one_fail_validation() {
        let yaml = r#"
name: demo
services:
  cache:
    image: redis:7
    deploy:
      replicas: 2
"#;
        let err = from_compose_str(yaml, None, Backend::Local, None).unwrap_err();
        assert!(err.to_string().contains("local backend has no HA"), "{err}");
    }

    #[test]
    fn backend_flag_is_honored() {
        let spec = from_compose_str(DEMO, Some("demoaws"), Backend::Aws, None).unwrap();
        assert_eq!(spec.project, "demoaws");
        assert_eq!(spec.backend, Backend::Aws);
    }

    #[test]
    fn mem_limit_maps_size() {
        let yaml = r#"
name: demo
services:
  cache:
    image: redis:7
    mem_limit: 512m
"#;
        let spec = from_compose_str(yaml, None, Backend::Local, None).unwrap();
        assert_eq!(spec.resource("cache").unwrap().size, Size::Medium);
    }

    #[test]
    fn docker_io_library_postgres_is_postgres() {
        let (kind, tag) = kind_from_image("docker.io/library/postgres:16").unwrap();
        assert_eq!(kind, Kind::Postgres);
        assert_eq!(tag.as_deref(), Some("16"));
    }

    #[test]
    fn mysql_image_maps_kind() {
        let (kind, tag) = kind_from_image("mysql:8").unwrap();
        assert_eq!(kind, Kind::Mysql);
        assert_eq!(tag.as_deref(), Some("8"));
        let yaml = r#"
name: demo
services:
  appmysql:
    image: mysql:8
    ports:
      - "127.0.0.1:3307:3306"
"#;
        let spec = from_compose_str(yaml, None, Backend::Local, None).unwrap();
        let r = spec.resource("appmysql").unwrap();
        assert_eq!(r.kind, Kind::Mysql);
        assert_eq!(r.port, Some(3307));
        assert_eq!(r.bind, Bind::Localhost);
    }
}
