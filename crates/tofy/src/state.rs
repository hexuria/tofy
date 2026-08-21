use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tofy_spec::{internal_host, Backend, Bind, Kind, Project, Resource, Size};

use crate::error::Result;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Planned,
    Emitted,
    Applied,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceState {
    pub kind: Kind,
    pub status: Status,
    pub image: String,
    pub port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default)]
    pub size: Size,
    #[serde(default)]
    pub bind: Bind,
    #[serde(default = "default_replicas")]
    pub replicas: u32,
    pub outputs: BTreeMap<String, String>,
}

fn default_replicas() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct State {
    pub project: String,
    #[serde(default)]
    pub backend: Backend,
    pub resources: BTreeMap<String, ResourceState>,
    /// Applying machine's public IPv4 as `a.b.c.d/32`. Used for the AWS
    /// security group. Not a secret. Absent on Local / Tofu and on S3-only stacks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applier_cidr: Option<String>,
}

impl State {
    pub fn path(root: &Path) -> PathBuf {
        root.join(".tofy").join("state.json")
    }

    pub fn load(root: &Path) -> Result<Self> {
        let p = Self::path(root);
        if !p.exists() {
            return Ok(Self::default());
        }
        Ok(serde_json::from_str(&std::fs::read_to_string(p)?)?)
    }

    pub fn save(&self, root: &Path) -> Result<()> {
        let dir = root.join(".tofy");
        std::fs::create_dir_all(&dir)?;
        let path = Self::path(root);
        let tmp = dir.join("state.json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        set_private(&tmp)?;
        std::fs::rename(&tmp, &path)?;
        set_private(&path)?;
        Ok(())
    }

    pub fn clear_resources(&mut self) {
        self.resources.clear();
        self.applier_cidr = None;
    }

    /// Rebuild a [`Project`] from persisted resource state (destroy / re-emit).
    pub fn as_project(&self) -> Project {
        let mut project = Project::new(&self.project);
        project.backend = self.backend;
        for (name, rs) in &self.resources {
            project.resources.push(Resource {
                name: name.clone(),
                kind: rs.kind,
                version: rs.version.clone(),
                port: Some(rs.port),
                size: rs.size,
                bind: rs.bind,
                replicas: rs.replicas,
            });
        }
        project
    }
}

pub fn set_private(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    let _ = path;
    Ok(())
}

pub fn docker_image(r: &Resource) -> String {
    match r.kind {
        Kind::Postgres => format!("postgres:{}", r.version_or_default()),
        Kind::Redis => format!("redis:{}", r.version_or_default()),
        Kind::Bucket => format!("minio/minio:{}", r.version_or_default()),
    }
}

fn aws_image(r: &Resource) -> String {
    match r.kind {
        Kind::Postgres => r.size.aws_rds_instance_class().to_string(),
        Kind::Redis => r.size.aws_elasticache_node_type().to_string(),
        Kind::Bucket => format!("s3:{}", r.size.aws_s3_storage_class()),
    }
}

pub fn generate_secret(len: usize) -> String {
    use rand::Rng;
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();
    (0..len)
        .map(|_| CHARS[rng.gen_range(0..CHARS.len())] as char)
        .collect()
}

fn existing_output<'a>(have: Option<&'a ResourceState>, key: &str) -> Option<String> {
    have.and_then(|h| h.outputs.get(key).cloned())
        .filter(|v| !v.is_empty())
}

/// Build the next state, reusing secrets already stored for a resource.
pub fn prepare_state(spec: &Project, current: &State) -> State {
    let mut resources = BTreeMap::new();
    for r in &spec.resources {
        let have = current.resources.get(&r.name);
        resources.insert(
            r.name.clone(),
            ResourceState {
                kind: r.kind,
                status: Status::Planned,
                image: match spec.backend {
                    Backend::Aws => aws_image(r),
                    Backend::Local | Backend::Tofu => docker_image(r),
                },
                port: r.port_or_default(),
                version: Some(r.version_or_default().to_string()),
                size: r.size,
                bind: r.bind,
                replicas: r.replicas_or_default(),
                outputs: outputs_for(spec, r, have),
            },
        );
    }
    State {
        project: spec.project.clone(),
        backend: spec.backend,
        resources,
        applier_cidr: if spec.backend == Backend::Aws {
            current.applier_cidr.clone()
        } else {
            None
        },
    }
}

pub fn outputs_for(
    spec: &Project,
    r: &Resource,
    have: Option<&ResourceState>,
) -> BTreeMap<String, String> {
    let _ = spec;
    let port = r.port_or_default();
    let internal_port = r.kind.internal_port();
    let in_host = internal_host(&r.name);
    let mut out = BTreeMap::new();
    out.insert("bind".into(), r.bind.as_ip().to_string());
    out.insert("size".into(), r.size.as_str().to_string());
    out.insert("replicas".into(), r.replicas_or_default().to_string());
    match spec.backend {
        Backend::Aws => aws_outputs_for(spec, r, have, &mut out),
        Backend::Local | Backend::Tofu => {
            local_outputs_for(r, have, port, internal_port, in_host, &mut out)
        }
    }
    out
}

fn local_outputs_for(
    r: &Resource,
    have: Option<&ResourceState>,
    port: u16,
    internal_port: u16,
    in_host: &str,
    out: &mut BTreeMap<String, String>,
) {
    match r.kind {
        Kind::Postgres => {
            let user = "tofy".to_string();
            let password = existing_output(have, "password").unwrap_or_else(|| generate_secret(32));
            let database = r.name.replace('-', "_");
            out.insert(
                "uri".into(),
                format!("postgres://{user}:{password}@127.0.0.1:{port}/{database}"),
            );
            out.insert(
                "internal_uri".into(),
                format!("postgres://{user}:{password}@{in_host}:{internal_port}/{database}"),
            );
            out.insert("user".into(), user);
            out.insert("password".into(), password);
            out.insert("database".into(), database);
            out.insert("host".into(), "127.0.0.1".into());
            out.insert("port".into(), port.to_string());
            out.insert("internal_host".into(), in_host.to_string());
            out.insert("internal_port".into(), internal_port.to_string());
        }
        Kind::Redis => {
            let password = existing_output(have, "password").unwrap_or_else(|| generate_secret(32));
            out.insert(
                "uri".into(),
                format!("redis://:{password}@127.0.0.1:{port}"),
            );
            out.insert(
                "internal_uri".into(),
                format!("redis://:{password}@{in_host}:{internal_port}"),
            );
            out.insert("password".into(), password);
            out.insert("host".into(), "127.0.0.1".into());
            out.insert("port".into(), port.to_string());
            out.insert("internal_host".into(), in_host.to_string());
            out.insert("internal_port".into(), internal_port.to_string());
        }
        Kind::Bucket => {
            let access_key =
                existing_output(have, "access_key").unwrap_or_else(|| generate_secret(16));
            let secret_key =
                existing_output(have, "secret_key").unwrap_or_else(|| generate_secret(32));
            out.insert("endpoint".into(), format!("http://127.0.0.1:{port}"));
            out.insert(
                "internal_endpoint".into(),
                format!("http://{in_host}:{internal_port}"),
            );
            out.insert("access_key".into(), access_key);
            out.insert("secret_key".into(), secret_key);
            out.insert("bucket".into(), r.name.clone());
            out.insert("host".into(), "127.0.0.1".into());
            out.insert("port".into(), port.to_string());
            out.insert("internal_host".into(), in_host.to_string());
            out.insert("internal_port".into(), internal_port.to_string());
        }
    }
}

fn aws_outputs_for(
    spec: &Project,
    r: &Resource,
    have: Option<&ResourceState>,
    out: &mut BTreeMap<String, String>,
) {
    let port = r.port_or_default();
    match r.kind {
        Kind::Postgres => {
            let user = "tofy".to_string();
            let password = existing_output(have, "password").unwrap_or_else(|| generate_secret(32));
            let database = r.name.replace('-', "_");
            let host = existing_output(have, "host").unwrap_or_default();
            if !host.is_empty() {
                out.insert(
                    "uri".into(),
                    format!("postgres://{user}:{password}@{host}:{port}/{database}"),
                );
                out.insert("host".into(), host);
            }
            out.insert("user".into(), user);
            out.insert("password".into(), password);
            out.insert("database".into(), database);
            out.insert("port".into(), port.to_string());
        }
        Kind::Redis => {
            let password = existing_output(have, "password").unwrap_or_else(|| generate_secret(32));
            let host = existing_output(have, "host").unwrap_or_default();
            if !host.is_empty() {
                out.insert("uri".into(), format!("redis://:{password}@{host}:{port}"));
                out.insert("host".into(), host);
            }
            out.insert("password".into(), password);
            out.insert("port".into(), port.to_string());
        }
        Kind::Bucket => {
            let bucket = existing_output(have, "bucket").unwrap_or_else(|| {
                crate::aws::s3_bucket_name(&spec.project, &r.name, &generate_secret(8))
            });
            let region = existing_output(have, "region")
                .or_else(crate::aws::region)
                .unwrap_or_default();
            out.insert("bucket".into(), bucket.clone());
            if !region.is_empty() {
                out.insert(
                    "endpoint".into(),
                    format!("https://{bucket}.s3.{region}.amazonaws.com"),
                );
                out.insert("region".into(), region);
            }
        }
    }
}

pub fn mark_applied(state: &mut State) {
    for r in state.resources.values_mut() {
        r.status = Status::Applied;
    }
}

pub fn mark_emitted(state: &mut State) {
    for r in state.resources.values_mut() {
        r.status = Status::Emitted;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tofy_spec::{Kind, Project, Resource};

    fn postgres_spec() -> Project {
        let mut p = Project::new("demo");
        p.resources.push(
            Resource::new("appdb", Kind::Postgres)
                .with_version("16")
                .with_port(5433),
        );
        p
    }

    #[test]
    fn secret_generated_once() {
        let spec = postgres_spec();
        let empty = State::default();
        let first = prepare_state(&spec, &empty);
        let pass1 = first.resources["appdb"].outputs["password"].clone();
        assert_eq!(pass1.len(), 32);
        assert!(!pass1.starts_with("tofy-"));
        assert_ne!(pass1, format!("tofy-{}-{}", spec.project, "appdb"));

        let second = prepare_state(&spec, &first);
        let pass2 = second.resources["appdb"].outputs["password"].clone();
        assert_eq!(pass1, pass2);
        assert_eq!(
            first.resources["appdb"].outputs["uri"],
            second.resources["appdb"].outputs["uri"]
        );
    }

    #[test]
    fn new_resource_gets_fresh_secret() {
        let spec = postgres_spec();
        let first = prepare_state(&spec, &State::default());
        let mut spec2 = spec.clone();
        spec2.resources.push(Resource::new("other", Kind::Postgres));
        let next = prepare_state(&spec2, &first);
        assert_eq!(
            first.resources["appdb"].outputs["password"],
            next.resources["appdb"].outputs["password"]
        );
        assert_ne!(
            next.resources["appdb"].outputs["password"],
            next.resources["other"].outputs["password"]
        );
    }

    #[test]
    fn host_uri_is_loopback_internal_uses_dns() {
        let spec = postgres_spec();
        let state = prepare_state(&spec, &State::default());
        let outs = &state.resources["appdb"].outputs;
        assert!(outs["uri"].contains("@127.0.0.1:5433/"));
        assert!(outs["internal_uri"].contains("@appdb:5432/"));
        assert_eq!(outs["host"], "127.0.0.1");
        assert_eq!(outs["internal_host"], "appdb");
        assert_eq!(outs["internal_port"], "5432");
        assert_eq!(outs["bind"], "127.0.0.1");
    }

    #[test]
    fn redis_password_generated_once_and_in_uri() {
        let mut spec = Project::new("demo");
        spec.resources.push(Resource::new("cache", Kind::Redis));
        let first = prepare_state(&spec, &State::default());
        let pass = first.resources["cache"].outputs["password"].clone();
        assert_eq!(pass.len(), 32);
        assert!(!pass.starts_with("tofy-"));
        assert!(first.resources["cache"].outputs["uri"]
            .contains(&format!("redis://:{pass}@127.0.0.1:")));
        let second = prepare_state(&spec, &first);
        assert_eq!(pass, second.resources["cache"].outputs["password"]);
        assert_eq!(
            first.resources["cache"].outputs["uri"],
            second.resources["cache"].outputs["uri"]
        );
    }

    #[test]
    fn prepare_copies_backend() {
        let mut spec = postgres_spec();
        spec.backend = Backend::Tofu;
        let state = prepare_state(&spec, &State::default());
        assert_eq!(state.backend, Backend::Tofu);
        assert_eq!(state.as_project().backend, Backend::Tofu);
        assert_eq!(state.as_project().resources[0].name, "appdb");
    }

    #[test]
    fn aws_outputs_are_iam_less_bucket_and_generated_secrets() {
        let mut spec = Project::new("demoaws");
        spec.backend = Backend::Aws;
        spec.resources
            .push(Resource::new("appdb", Kind::Postgres).with_port(25432));
        spec.resources.push(Resource::new("cache", Kind::Redis));
        spec.resources.push(Resource::new("uploads", Kind::Bucket));
        let first = prepare_state(&spec, &State::default());
        let db = &first.resources["appdb"].outputs;
        assert_eq!(db["password"].len(), 32);
        assert!(!db.contains_key("host"), "{db:?}");
        assert!(!db.contains_key("uri"), "{db:?}");
        assert_eq!(db["user"], "tofy");
        assert_eq!(first.resources["appdb"].image, "db.t4g.micro");
        let cache = &first.resources["cache"].outputs;
        assert_eq!(cache["password"].len(), 32);
        assert!(!cache.contains_key("uri"));
        let files = &first.resources["uploads"].outputs;
        assert!(
            files["bucket"].starts_with("tofy-demoaws-uploads-"),
            "{files:?}"
        );
        assert!(!files.contains_key("access_key"), "{files:?}");
        assert!(!files.contains_key("secret_key"), "{files:?}");
        let second = prepare_state(&spec, &first);
        assert_eq!(
            first.resources["appdb"].outputs["password"],
            second.resources["appdb"].outputs["password"]
        );
        assert_eq!(
            first.resources["uploads"].outputs["bucket"],
            second.resources["uploads"].outputs["bucket"]
        );
        assert!(first.applier_cidr.is_none());
        let mut with_cidr = first.clone();
        with_cidr.applier_cidr = Some("203.0.113.10/32".into());
        let third = prepare_state(&spec, &with_cidr);
        assert_eq!(third.applier_cidr.as_deref(), Some("203.0.113.10/32"));
    }

    #[test]
    fn state_file_mode_0600() {
        let dir = tempfile::tempdir().unwrap();
        let spec = postgres_spec();
        let state = prepare_state(&spec, &State::default());
        state.save(dir.path()).unwrap();
        let path = State::path(dir.path());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }
}
