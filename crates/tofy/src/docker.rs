use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use tofy_spec::{
    docker_network, replica_alias, replica_container, replica_volume, Kind, Project, Resource,
};

use crate::error::{Error, Result};
use crate::state::{docker_image, ResourceState};

pub fn available() -> bool {
    Command::new("docker")
        .args(["info", "--format", "{{.ServerVersion}}"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn container_running(name: &str) -> bool {
    let out = Command::new("docker")
        .args(["inspect", "-f", "{{.State.Running}}", name])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim() == "true",
        _ => false,
    }
}

pub fn container_exists(name: &str) -> bool {
    Command::new("docker")
        .args(["inspect", name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Live Docker facts used by plan to detect drift vs `.tofy/state.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContainerLive {
    Missing,
    Present(ContainerFacts),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ContainerFacts {
    pub running: bool,
    pub image: Option<String>,
    pub image_id: Option<String>,
    pub host_ip: Option<String>,
    pub host_port: Option<u16>,
    pub project: Option<String>,
    pub resource: Option<String>,
}

/// Inspect a container. Missing (or inspect failure) is [`ContainerLive::Missing`].
pub fn inspect_live(name: &str, internal_port: u16) -> ContainerLive {
    let out = Command::new("docker")
        .args(["inspect", name])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();
    match out {
        Ok(o) if o.status.success() => parse_inspect(&o.stdout, internal_port),
        _ => ContainerLive::Missing,
    }
}

/// Fold every replica into one live status for plan. Port/bind come from replica 0.
/// A missing or stopped extra replica marks the resource not running so apply heals.
pub fn inspect_replicas(project: &str, r: &Resource) -> ContainerLive {
    let n = r.replicas_or_default();
    let internal = r.kind.internal_port();
    let mut status = inspect_live(&replica_container(project, &r.name, 0), internal);
    for i in 1..n {
        match inspect_live(&replica_container(project, &r.name, i), internal) {
            ContainerLive::Missing => {
                if let ContainerLive::Present(facts) = &mut status {
                    facts.running = false;
                }
            }
            ContainerLive::Present(extra) if !extra.running => {
                if let ContainerLive::Present(facts) = &mut status {
                    facts.running = false;
                }
            }
            ContainerLive::Present(_) => {}
        }
    }
    status
}

fn parse_inspect(bytes: &[u8], internal_port: u16) -> ContainerLive {
    let v: serde_json::Value = match serde_json::from_slice(bytes) {
        Ok(v) => v,
        Err(_) => return ContainerLive::Missing,
    };
    let obj = if let Some(arr) = v.as_array() {
        match arr.first() {
            Some(o) => o,
            None => return ContainerLive::Missing,
        }
    } else {
        &v
    };
    let running = obj
        .pointer("/State/Running")
        .and_then(|x| x.as_bool())
        .unwrap_or(false);
    let image = obj
        .pointer("/Config/Image")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let image_id = obj
        .pointer("/Image")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let labels = &obj["Config"]["Labels"];
    let project = labels
        .get("tofy.project")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string());
    let resource = labels
        .get("tofy.resource")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string());
    let (host_ip, host_port) = published_port(obj, internal_port);
    ContainerLive::Present(ContainerFacts {
        running,
        image,
        image_id,
        host_ip,
        host_port,
        project,
        resource,
    })
}

fn published_port(obj: &serde_json::Value, internal_port: u16) -> (Option<String>, Option<u16>) {
    let key = format!("{internal_port}/tcp");
    binding_for(&obj["HostConfig"]["PortBindings"], &key)
        .or_else(|| binding_for(&obj["NetworkSettings"]["Ports"], &key))
        .unwrap_or((None, None))
}

fn binding_for(map: &serde_json::Value, key: &str) -> Option<(Option<String>, Option<u16>)> {
    let first = map.get(key)?.as_array()?.first()?;
    let ip = first.get("HostIp").and_then(|x| x.as_str()).map(|s| {
        if s.is_empty() {
            "0.0.0.0".to_string()
        } else {
            s.to_string()
        }
    });
    let port = first
        .get("HostPort")
        .and_then(|x| x.as_str())
        .and_then(|s| s.parse().ok());
    Some((ip, port))
}

/// Compare the desired image tag (e.g. `postgres:16`) to live inspect fields.
/// Tofu-managed containers often store a `sha256:…` image id; those still match
/// when `docker image inspect` resolves the tag to the same id.
pub fn image_matches(want: &str, facts: &ContainerFacts) -> bool {
    if facts.image.as_deref() == Some(want) {
        return true;
    }
    let want_id = image_id(want);
    if let Some(ref want_id) = want_id {
        if facts.image_id.as_deref() == Some(want_id.as_str())
            || facts.image.as_deref() == Some(want_id.as_str())
        {
            return true;
        }
        return false;
    }
    // Tag is not local, so we cannot prove a mismatch against a digest.
    facts
        .image
        .as_deref()
        .is_some_and(|s| s.starts_with("sha256:"))
}

fn image_id(name: &str) -> Option<String> {
    let out = Command::new("docker")
        .args(["image", "inspect", "-f", "{{.Id}}", name])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

pub fn remove_container(name: &str) -> Result<()> {
    let _ = Command::new("docker")
        .args(["rm", "-f", name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    Ok(())
}

pub fn remove_volume(name: &str) -> Result<()> {
    let _ = Command::new("docker")
        .args(["volume", "rm", "-f", name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    Ok(())
}

fn run_checked(mut cmd: Command) -> Result<()> {
    let out = cmd.output()?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(Error::Engine(format!("docker failed: {}", stderr.trim())));
    }
    Ok(())
}

pub fn ensure_network(project: &str) -> Result<String> {
    let name = docker_network(project);
    if network_exists(&name) {
        return Ok(name);
    }
    let mut cmd = Command::new("docker");
    cmd.args([
        "network",
        "create",
        "--label",
        &format!("tofy.project={project}"),
        &name,
    ]);
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::piped());
    run_checked(cmd)?;
    Ok(name)
}

pub fn network_exists(name: &str) -> bool {
    Command::new("docker")
        .args(["network", "inspect", name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn destroy_network(project: &str) -> Result<()> {
    let name = docker_network(project);
    let _ = Command::new("docker")
        .args(["network", "rm", &name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    Ok(())
}

fn remove_labeled(project: &str, resource: &str) -> Result<()> {
    let out = Command::new("docker")
        .args([
            "ps",
            "-aq",
            "--filter",
            &format!("label=tofy.project={project}"),
            "--filter",
            &format!("label=tofy.resource={resource}"),
        ])
        .output()?;
    let ids = String::from_utf8_lossy(&out.stdout);
    for id in ids.split_whitespace() {
        remove_container(id)?;
    }
    Ok(())
}

pub fn start_resource(spec: &Project, r: &Resource, rs: &ResourceState) -> Result<()> {
    ensure_network(&spec.project)?;
    remove_labeled(&spec.project, &r.name)?;
    let n = r.replicas_or_default();
    for i in 0..n {
        start_one(spec, r, rs, i)?;
    }
    for i in 0..n {
        ready_replica(r, rs, &replica_container(&spec.project, &r.name, i), i)?;
    }
    Ok(())
}

fn start_one(spec: &Project, r: &Resource, rs: &ResourceState, index: u32) -> Result<()> {
    let name = replica_container(&spec.project, &r.name, index);
    let vol = replica_volume(&spec.project, &r.name, index);
    let net = docker_network(&spec.project);
    let alias = replica_alias(&r.name, index);
    let replica_label = format!("tofy.replica={}", index + 1);

    if matches!(r.kind, Kind::Postgres | Kind::Mysql | Kind::Bucket) {
        let mut vc = Command::new("docker");
        vc.args(["volume", "create", &vol]);
        vc.stdout(Stdio::null());
        vc.stderr(Stdio::piped());
        run_checked(vc)?;
    }

    let mut cmd = Command::new("docker");
    cmd.args([
        "run",
        "-d",
        "--name",
        &name,
        "--hostname",
        &alias,
        "--network",
        &net,
        "--network-alias",
        &alias,
        "--restart",
        "unless-stopped",
        "--memory",
        r.size.docker_memory(),
        "--cpus",
        r.size.docker_cpus(),
        "--label",
        &format!("tofy.project={}", spec.project),
        "--label",
        &format!("tofy.resource={}", r.name),
        "--label",
        &replica_label,
    ]);
    if index == 0 {
        let host_port = rs.port;
        let internal = r.kind.internal_port();
        cmd.args(["-p", &format!("{}:{host_port}:{internal}", r.bind.as_ip())]);
    }

    match r.kind {
        Kind::Postgres => {
            let user = rs.outputs.get("user").map(String::as_str).unwrap_or("tofy");
            let password = rs
                .outputs
                .get("password")
                .ok_or_else(|| Error::Engine("postgres password missing from state".into()))?;
            let database = rs
                .outputs
                .get("database")
                .map(String::as_str)
                .unwrap_or(r.name.as_str());
            cmd.args(["-e", &format!("POSTGRES_USER={user}")]);
            cmd.args(["-e", &format!("POSTGRES_PASSWORD={password}")]);
            cmd.args(["-e", &format!("POSTGRES_DB={database}")]);
            cmd.args(["-v", &format!("{vol}:/var/lib/postgresql/data")]);
            cmd.arg(docker_image(r));
        }
        Kind::Mysql => {
            let user = rs.outputs.get("user").map(String::as_str).unwrap_or("tofy");
            let password = rs
                .outputs
                .get("password")
                .ok_or_else(|| Error::Engine("mysql password missing from state".into()))?;
            let database = rs
                .outputs
                .get("database")
                .map(String::as_str)
                .unwrap_or(r.name.as_str());
            cmd.args(["-e", &format!("MYSQL_USER={user}")]);
            cmd.args(["-e", &format!("MYSQL_PASSWORD={password}")]);
            cmd.args(["-e", &format!("MYSQL_DATABASE={database}")]);
            cmd.args(["-e", &format!("MYSQL_ROOT_PASSWORD={password}")]);
            cmd.args(["-v", &format!("{vol}:/var/lib/mysql")]);
            cmd.arg(docker_image(r));
        }
        Kind::Redis => {
            let password = rs
                .outputs
                .get("password")
                .ok_or_else(|| Error::Engine("redis password missing from state".into()))?;
            cmd.arg(docker_image(r));
            cmd.args(["redis-server", "--requirepass", password]);
        }
        Kind::Bucket => {
            let access = rs
                .outputs
                .get("access_key")
                .ok_or_else(|| Error::Engine("bucket access_key missing from state".into()))?;
            let secret = rs
                .outputs
                .get("secret_key")
                .ok_or_else(|| Error::Engine("bucket secret_key missing from state".into()))?;
            cmd.args(["-e", &format!("MINIO_ROOT_USER={access}")]);
            cmd.args(["-e", &format!("MINIO_ROOT_PASSWORD={secret}")]);
            cmd.args(["-v", &format!("{vol}:/data")]);
            cmd.arg(docker_image(r));
            cmd.args(["server", "/data", "--console-address", ":9001"]);
        }
        Kind::Secret => {
            return Err(Error::Engine(
                "secret is state-only; it has no container".into(),
            ));
        }
    }

    run_checked(cmd)
}

pub fn ensure_running(spec: &Project, r: &Resource, rs: &ResourceState) -> Result<()> {
    ensure_network(&spec.project)?;
    let n = r.replicas_or_default();
    for i in 0..n {
        let name = replica_container(&spec.project, &r.name, i);
        if !container_running(&name) {
            if container_exists(&name) {
                let mut cmd = Command::new("docker");
                cmd.args(["start", &name]);
                run_checked(cmd)?;
            } else {
                start_one(spec, r, rs, i)?;
            }
        }
        ready_replica(r, rs, &name, i)?;
    }
    Ok(())
}

pub fn ready_resource(r: &Resource, rs: &ResourceState, container: &str) -> Result<()> {
    ready_replica(r, rs, container, 0)
}

pub fn ready_replica(r: &Resource, rs: &ResourceState, container: &str, index: u32) -> Result<()> {
    let host_port = index == 0;
    match r.kind {
        Kind::Postgres => wait_for_postgres(container, rs.port, host_port),
        Kind::Mysql => {
            let password = rs
                .outputs
                .get("password")
                .ok_or_else(|| Error::Engine("mysql password missing from state".into()))?;
            wait_for_mysql(container, rs.port, password, host_port)
        }
        Kind::Redis => {
            let password = rs
                .outputs
                .get("password")
                .ok_or_else(|| Error::Engine("redis password missing from state".into()))?;
            wait_for_redis(container, rs.port, password, host_port)
        }
        Kind::Bucket => {
            if !host_port {
                return Err(Error::Engine(
                    "bucket has no HA: extra replicas are not started".into(),
                ));
            }
            wait_tcp("object store", rs.port)?;
            crate::s3::wait_for_object_store(rs.port)?;
            let access = rs
                .outputs
                .get("access_key")
                .ok_or_else(|| Error::Engine("bucket access_key missing from state".into()))?;
            let secret = rs
                .outputs
                .get("secret_key")
                .ok_or_else(|| Error::Engine("bucket secret_key missing from state".into()))?;
            crate::s3::ensure_bucket(rs.port, access, secret, &r.name)
        }
        Kind::Secret => Ok(()),
    }
}

pub fn destroy_resource(project: &str, name: &str, replicas: u32) -> Result<()> {
    let _ = remove_labeled(project, name);
    let n = replicas.max(1);
    for i in 0..n {
        remove_container(&replica_container(project, name, i))?;
        remove_volume(&replica_volume(project, name, i))?;
    }
    Ok(())
}

pub fn wait_for_postgres(container: &str, port: u16, host_port: bool) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if pg_ready(container, port, host_port) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(Error::Engine(format!(
                "Postgres on {container} did not accept connections within 60s"
            )));
        }
        thread::sleep(Duration::from_millis(400));
    }
}

pub fn wait_for_mysql(container: &str, port: u16, password: &str, host_port: bool) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if mysql_ready(container, port, password, host_port) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(Error::Engine(format!(
                "Mysql on {container} did not accept connections within 60s"
            )));
        }
        thread::sleep(Duration::from_millis(400));
    }
}

pub fn wait_for_redis(container: &str, port: u16, password: &str, host_port: bool) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if redis_container_ready(container, password) || (host_port && redis_ready(port, password))
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(Error::Engine(format!(
                "Redis on {container} did not accept AUTH within 60s"
            )));
        }
        thread::sleep(Duration::from_millis(400));
    }
}

fn wait_tcp(label: &str, port: u16) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if tcp_open(port) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(Error::Engine(format!(
                "{label} on 127.0.0.1:{port} did not accept connections within 60s"
            )));
        }
        thread::sleep(Duration::from_millis(400));
    }
}

fn redis_ready(port: u16, password: &str) -> bool {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = match TcpStream::connect_timeout(&addr, Duration::from_millis(200)) {
        Ok(s) => s,
        Err(_) => return false,
    };
    stream
        .set_read_timeout(Some(Duration::from_millis(400)))
        .ok();
    stream
        .set_write_timeout(Some(Duration::from_millis(400)))
        .ok();
    if stream
        .write_all(format!("AUTH {password}\r\nPING\r\n").as_bytes())
        .is_err()
    {
        return false;
    }
    let mut buf = [0u8; 128];
    match stream.read(&mut buf) {
        Ok(n) if n > 0 => {
            let s = String::from_utf8_lossy(&buf[..n]);
            s.contains("+PONG")
        }
        _ => false,
    }
}

fn pg_ready(container: &str, port: u16, host_port: bool) -> bool {
    let exec_ok = Command::new("docker")
        .args([
            "exec",
            container,
            "pg_isready",
            "-h",
            "127.0.0.1",
            "-p",
            "5432",
            "-U",
            "tofy",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    exec_ok || (host_port && tcp_open(port))
}

fn mysql_ready(container: &str, port: u16, password: &str, host_port: bool) -> bool {
    let pass = format!("-p{password}");
    let exec_ok = Command::new("docker")
        .args([
            "exec",
            container,
            "mysqladmin",
            "ping",
            "-h",
            "127.0.0.1",
            "-uroot",
            &pass,
            "--silent",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    exec_ok || (host_port && tcp_open(port))
}

fn redis_container_ready(container: &str, password: &str) -> bool {
    Command::new("docker")
        .args(["exec", container, "redis-cli", "-a", password, "ping"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).contains("PONG"))
        .unwrap_or(false)
}

pub fn tcp_open(port: u16) -> bool {
    TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_millis(200),
    )
    .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_tag_matches_without_docker() {
        let facts = ContainerFacts {
            running: true,
            image: Some("redis:7".into()),
            ..Default::default()
        };
        assert!(image_matches("redis:7", &facts));
        assert!(!image_matches("redis:6", &facts));
    }

    #[test]
    fn parse_inspect_stopped_port_and_labels() {
        let raw = r#"[{
            "State": {"Running": false},
            "Config": {
                "Image": "redis:7",
                "Labels": {"tofy.project": "demo", "tofy.resource": "cache"}
            },
            "Image": "sha256:abc",
            "HostConfig": {
                "PortBindings": {
                    "6379/tcp": [{"HostIp": "127.0.0.1", "HostPort": "6379"}]
                }
            },
            "NetworkSettings": {"Ports": {}}
        }]"#;
        match parse_inspect(raw.as_bytes(), 6379) {
            ContainerLive::Present(f) => {
                assert!(!f.running);
                assert_eq!(f.image.as_deref(), Some("redis:7"));
                assert_eq!(f.host_ip.as_deref(), Some("127.0.0.1"));
                assert_eq!(f.host_port, Some(6379));
                assert_eq!(f.project.as_deref(), Some("demo"));
                assert_eq!(f.resource.as_deref(), Some("cache"));
            }
            other => panic!("{other:?}"),
        }
    }
}
