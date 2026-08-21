//! Constrained Docker Compose subset and docker-provider OpenTofu JSON → JSON IR.
//!
//! This is an importer, not a yaml write path and not auto-load. Unknown
//! images fail. Secrets in Compose env / tofu env are not copied into the spec.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;
use tofy_spec::{Backend, Bind, Kind, Project, Resource, Size};

use crate::error::{Error, Result};

const IMAGE_ALIASES: &str =
    "postgres, postgresql, mysql, mariadb, redis, minio/minio, bitnami/postgres, bitnami/postgresql, bitnami/mysql, bitnami/mariadb, and bitnami/redis";

#[derive(Debug, Deserialize)]
struct ComposeFile {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    services: BTreeMap<String, ComposeService>,
    /// Named extra networks. Warned and ignored; not a failure.
    #[serde(default)]
    networks: Option<serde_yaml::Value>,
    /// Named extra volumes. Warned and ignored; not a failure.
    #[serde(default)]
    volumes: Option<serde_yaml::Value>,
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
    /// Warned and ignored; not a failure.
    #[serde(default)]
    depends_on: Option<serde_yaml::Value>,
    /// Service networks besides the ones we already ignore. Warned; not a failure.
    #[serde(default)]
    networks: Option<serde_yaml::Value>,
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
    if file.networks.is_some() {
        eprintln!("warning: compose top-level networks: ignored");
    }
    if file.volumes.is_some() {
        eprintln!("warning: compose top-level volumes: ignored");
    }
    if file.services.is_empty() {
        return Err(Error::Usage("compose has no services".into()));
    }
    let project_name = resolve_project(project, file.name.as_deref(), source)?;
    let mut spec = Project::new(project_name);
    spec.backend = backend;
    for (name, svc) in file.services {
        if svc.depends_on.is_some() {
            eprintln!("warning: compose service {name} depends_on ignored");
        }
        if svc.networks.is_some() {
            eprintln!("warning: compose service {name} networks ignored");
        }
        spec.resources.push(service_to_resource(&name, &svc)?);
    }
    spec.validate()?;
    Ok(spec)
}

/// Parse docker-provider OpenTofu JSON (`main.tf.json`) into a validated [`Project`].
/// Does not run tofu and does not apply. AWS-provider JSON is rejected.
pub fn from_tofu_file(path: &Path, project: Option<&str>, backend: Backend) -> Result<Project> {
    let raw = std::fs::read_to_string(path)?;
    from_tofu_str(&raw, project, backend)
}

/// Parse docker-provider OpenTofu JSON text into a validated [`Project`].
pub fn from_tofu_str(json: &str, project: Option<&str>, backend: Backend) -> Result<Project> {
    let value: Value = serde_json::from_str(json)?;
    from_tofu_value(&value, project, backend)
}

fn from_tofu_value(value: &Value, project: Option<&str>, backend: Backend) -> Result<Project> {
    if is_aws_opentofu(value) {
        return Err(Error::Usage(
            "AWS OpenTofu JSON cannot be imported; importer maps docker-provider JSON only".into(),
        ));
    }
    let containers = value
        .get("resource")
        .and_then(|r| r.get("docker_container"))
        .and_then(|c| c.as_object())
        .ok_or_else(|| {
            Error::Usage(
                "no docker_container resource; importer maps docker-provider OpenTofu JSON only"
                    .into(),
            )
        })?;
    if containers.is_empty() {
        return Err(Error::Usage(
            "no docker_container resource; importer maps docker-provider OpenTofu JSON only".into(),
        ));
    }
    let project_name = resolve_tofu_project(project, value)?;
    let mut spec = Project::new(project_name);
    spec.backend = backend;
    for (key, container) in containers {
        spec.resources
            .push(container_to_resource(value, key, container)?);
    }
    spec.validate()?;
    Ok(spec)
}

fn is_aws_opentofu(value: &Value) -> bool {
    let has_aws_provider = value
        .get("terraform")
        .and_then(|t| t.get("required_providers"))
        .and_then(|p| p.as_object())
        .is_some_and(|p| p.contains_key("aws"));
    if has_aws_provider {
        return true;
    }
    value
        .get("resource")
        .and_then(|r| r.as_object())
        .is_some_and(|r| r.keys().any(|k| k.starts_with("aws_")))
}

fn resolve_tofu_project(explicit: Option<&str>, value: &Value) -> Result<String> {
    if let Some(p) = explicit.map(str::trim).filter(|s| !s.is_empty()) {
        return Ok(p.to_string());
    }
    let stack = value
        .get("resource")
        .and_then(|r| r.get("docker_network"))
        .and_then(|n| n.get("stack"));
    if let Some(stack) = stack {
        if let Some(p) = label_in(stack, "tofy.project")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
        {
            return Ok(p);
        }
        if let Some(name) = stack.get("name").and_then(|v| v.as_str()) {
            let stripped = name
                .strip_prefix("tofy-")
                .unwrap_or(name)
                .trim()
                .to_string();
            if !stripped.is_empty() {
                return Ok(stripped);
            }
        }
    }
    Err(Error::Usage(
        "tofu import needs --project or docker_network.stack".into(),
    ))
}

fn container_to_resource(root: &Value, key: &str, container: &Value) -> Result<Resource> {
    let name = resource_name(container, key);
    let image = docker_image_name(root, key)?;
    let (kind, tag) = kind_from_image(image)?;
    let mut r = Resource::new(name, kind);
    if let Some(tag) = tag {
        r.version = Some(tag);
    }
    if let Some((bind, port)) = tofu_first_port(container)? {
        r.bind = bind;
        r.port = Some(port);
    }
    r.size = size_from_memory_mb(container.get("memory"));
    Ok(r)
}

fn resource_name(container: &Value, key: &str) -> String {
    if let Some(name) = label_in(container, "tofy.resource").filter(|s| !s.is_empty()) {
        return name;
    }
    if let Some(host) = container
        .get("hostname")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return host.to_string();
    }
    key.to_string()
}

fn docker_image_name<'a>(root: &'a Value, key: &str) -> Result<&'a str> {
    root.get("resource")
        .and_then(|r| r.get("docker_image"))
        .and_then(|imgs| imgs.get(key))
        .and_then(|img| img.get("name"))
        .and_then(|n| n.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            Error::Usage(format!(
                "docker_container {key} has no docker_image.{key}.name"
            ))
        })
}

fn tofu_first_port(container: &Value) -> Result<Option<(Bind, u16)>> {
    let Some(port0) = container
        .get("ports")
        .and_then(|p| p.as_array())
        .and_then(|a| a.first())
    else {
        return Ok(None);
    };
    let Some(external) = port0.get("external") else {
        return Ok(None);
    };
    let port = json_u16(external)?;
    let ip = port0.get("ip").and_then(|v| v.as_str()).unwrap_or("");
    Ok(Some((bind_from_ip(ip), port)))
}

fn json_u16(v: &Value) -> Result<u16> {
    match v {
        Value::Number(n) => n
            .as_u64()
            .and_then(|n| u16::try_from(n).ok())
            .ok_or_else(|| Error::Usage(format!("unsupported port {v}"))),
        Value::String(s) => s
            .parse()
            .map_err(|_| Error::Usage(format!("unsupported port {s:?}"))),
        _ => Err(Error::Usage(format!("unsupported port {v}"))),
    }
}

fn size_from_memory_mb(v: Option<&Value>) -> Size {
    let Some(n) = v.and_then(json_u64) else {
        return Size::Small;
    };
    match n {
        256 => Size::Small,
        512 => Size::Medium,
        1024 => Size::Large,
        _ => Size::Small,
    }
}

fn json_u64(v: &Value) -> Option<u64> {
    match v {
        Value::Number(n) => n
            .as_u64()
            .or_else(|| n.as_i64().and_then(|i| u64::try_from(i).ok())),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn label_in(obj: &Value, key: &str) -> Option<String> {
    let labels = obj.get("labels")?;
    let arr = labels.as_array()?;
    for item in arr {
        let Some(label) = item.get("label").and_then(|v| v.as_str()) else {
            continue;
        };
        if label == key {
            return item
                .get("value")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
        }
    }
    None
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
            "unknown compose image {image:?}; importer maps {IMAGE_ALIASES} only"
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
    // Last path segment: official postgres/mysql/redis/mariadb, bitnami/*, and */mariadb.
    let base = name.rsplit('/').next().unwrap_or(name);
    match base {
        "postgres" | "postgresql" => Some(Kind::Postgres),
        "mysql" | "mariadb" => Some(Kind::Mysql),
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
    use crate::emit::terraform_json;
    use crate::state::{prepare_state, State};

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
        assert!(msg.contains("mariadb"), "{msg}");
        assert!(msg.contains("bitnami/redis"), "{msg}");
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

    #[test]
    fn bitnami_and_mariadb_images_map() {
        let (kind, tag) = kind_from_image("bitnami/redis:7.2").unwrap();
        assert_eq!(kind, Kind::Redis);
        assert_eq!(tag.as_deref(), Some("7.2"));
        let (kind, tag) = kind_from_image("bitnami/postgresql:16").unwrap();
        assert_eq!(kind, Kind::Postgres);
        assert_eq!(tag.as_deref(), Some("16"));
        let (kind, _) = kind_from_image("bitnami/postgres:16").unwrap();
        assert_eq!(kind, Kind::Postgres);
        let (kind, tag) = kind_from_image("mariadb:11").unwrap();
        assert_eq!(kind, Kind::Mysql);
        assert_eq!(tag.as_deref(), Some("11"));
        let (kind, _) = kind_from_image("bitnami/mysql:8.0").unwrap();
        assert_eq!(kind, Kind::Mysql);
        let (kind, _) = kind_from_image("bitnami/mariadb:11").unwrap();
        assert_eq!(kind, Kind::Mysql);
        let (kind, _) = kind_from_image("docker.io/library/mariadb:11").unwrap();
        assert_eq!(kind, Kind::Mysql);
        let (kind, _) = kind_from_image("myorg/mariadb:11").unwrap();
        assert_eq!(kind, Kind::Mysql);

        let yaml = r#"
name: demo
services:
  cache:
    image: bitnami/redis:7.2
  appdb:
    image: bitnami/postgresql:16
  appmysql:
    image: mariadb:11
"#;
        let spec = from_compose_str(yaml, None, Backend::Local, None).unwrap();
        assert_eq!(spec.resource("cache").unwrap().kind, Kind::Redis);
        assert_eq!(spec.resource("appdb").unwrap().kind, Kind::Postgres);
        assert_eq!(spec.resource("appmysql").unwrap().kind, Kind::Mysql);
    }

    #[test]
    fn depends_on_and_extra_networks_volumes_succeed_without_passwords() {
        let yaml = r#"
name: demo
networks:
  extra: {}
volumes:
  data: {}
services:
  cache:
    image: redis:7
    depends_on:
      - appdb
    networks:
      extra: {}
    environment:
      REDIS_PASSWORD: supersecret
    ports:
      - "127.0.0.1:6379:6379"
  appdb:
    image: postgres:16
    environment:
      POSTGRES_PASSWORD: supersecret
    ports:
      - "127.0.0.1:5433:5432"
"#;
        let spec = from_compose_str(yaml, None, Backend::Local, None).unwrap();
        assert_eq!(spec.resources.len(), 2);
        assert_eq!(spec.resource("cache").unwrap().kind, Kind::Redis);
        assert_eq!(spec.resource("appdb").unwrap().kind, Kind::Postgres);
        let json = spec.to_json_pretty().unwrap();
        assert!(!json.to_ascii_lowercase().contains("password"), "{json}");
        assert!(!json.contains("supersecret"), "{json}");
        assert!(!json.contains("POSTGRES_"), "{json}");
    }

    #[test]
    fn tofu_json_round_trip() {
        let mut spec = Project::new("demo");
        spec.resources.push(
            Resource::new("appdb", Kind::Postgres)
                .with_port(5433)
                .with_size(Size::Medium),
        );
        spec.resources.push(
            Resource::new("cache", Kind::Redis)
                .with_port(6379)
                .with_bind(Bind::All),
        );
        spec.resources
            .push(Resource::new("uploads", Kind::Bucket).with_port(9000));
        let state = prepare_state(&spec, &State::default());
        let tf = terraform_json(&spec, &state);
        let raw = serde_json::to_string_pretty(&tf).unwrap();
        assert!(
            raw.contains("POSTGRES_PASSWORD") || raw.to_ascii_lowercase().contains("password"),
            "emitted tofu JSON should contain secrets so import is proven to drop them"
        );
        assert!(
            raw.contains("${docker_image.appdb.image_id}"),
            "must look up docker_image.name, not the interpolation"
        );

        let imported = from_tofu_str(&raw, None, Backend::Local).unwrap();
        assert_eq!(imported.project, "demo");
        assert_eq!(imported.backend, Backend::Local);
        assert_eq!(imported.resources.len(), 3);
        let appdb = imported.resource("appdb").unwrap();
        assert_eq!(appdb.kind, Kind::Postgres);
        assert_eq!(appdb.port, Some(5433));
        assert_eq!(appdb.bind, Bind::Localhost);
        assert_eq!(appdb.size, Size::Medium);
        let cache = imported.resource("cache").unwrap();
        assert_eq!(cache.kind, Kind::Redis);
        assert_eq!(cache.port, Some(6379));
        assert_eq!(cache.bind, Bind::All);
        let uploads = imported.resource("uploads").unwrap();
        assert_eq!(uploads.kind, Kind::Bucket);
        assert_eq!(uploads.port, Some(9000));
        let json = imported.to_json_pretty().unwrap();
        assert!(!json.to_ascii_lowercase().contains("password"), "{json}");
        assert!(!json.contains("POSTGRES_"), "{json}");
        assert!(!json.contains("supersecret"), "{json}");
    }

    #[test]
    fn tofu_aws_shaped_json_errors() {
        let providers = r#"{
            "terraform": { "required_providers": { "aws": { "source": "hashicorp/aws" } } },
            "resource": { "docker_container": { "appdb": { "hostname": "appdb" } } }
        }"#;
        let err = from_tofu_str(providers, Some("demo"), Backend::Aws).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("AWS"), "{msg}");
        assert!(msg.contains("docker-provider"), "{msg}");

        let rds = r#"{
            "resource": { "aws_db_instance": { "appdb": { "engine": "postgres" } } }
        }"#;
        let err = from_tofu_str(rds, Some("demo"), Backend::Local).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("AWS"), "{msg}");
        assert!(msg.contains("docker-provider"), "{msg}");
    }

    #[test]
    fn tofu_missing_docker_container_errors() {
        let json = r#"{
            "terraform": { "required_providers": { "docker": { "source": "kreuzwerker/docker" } } },
            "resource": { "docker_network": { "stack": { "name": "tofy-demo" } } }
        }"#;
        let err = from_tofu_str(json, Some("demo"), Backend::Local).unwrap_err();
        assert!(err.to_string().contains("docker_container"), "{err}");
    }
}
