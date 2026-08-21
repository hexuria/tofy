use std::net::TcpStream;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use tofy_spec::{docker_network, replica_container, replica_volume, Kind, Project, Resource};

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
    start_one(spec, r, rs)?;
    ready_resource(r, rs, &replica_container(&spec.project, &r.name, 0))?;
    Ok(())
}

fn start_one(spec: &Project, r: &Resource, rs: &ResourceState) -> Result<()> {
    let name = replica_container(&spec.project, &r.name, 0);
    let vol = replica_volume(&spec.project, &r.name, 0);
    let net = docker_network(&spec.project);
    let hostname = r.name.clone();

    if matches!(r.kind, Kind::Postgres | Kind::Bucket) {
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
        &hostname,
        "--network",
        &net,
        "--network-alias",
        &r.name,
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
        "tofy.replica=1",
    ]);
    let host_port = rs.port;
    let internal = r.kind.internal_port();
    cmd.args(["-p", &format!("{}:{host_port}:{internal}", r.bind.as_ip())]);

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
        Kind::Redis => {
            cmd.arg(docker_image(r));
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
    }

    run_checked(cmd)
}

pub fn ensure_running(spec: &Project, r: &Resource, rs: &ResourceState) -> Result<()> {
    ensure_network(&spec.project)?;
    let name = replica_container(&spec.project, &r.name, 0);
    if !container_running(&name) {
        if container_exists(&name) {
            let mut cmd = Command::new("docker");
            cmd.args(["start", &name]);
            run_checked(cmd)?;
        } else {
            start_one(spec, r, rs)?;
        }
    }
    ready_resource(r, rs, &name)?;
    Ok(())
}

fn ready_resource(r: &Resource, rs: &ResourceState, container: &str) -> Result<()> {
    match r.kind {
        Kind::Postgres => wait_for_postgres(container, rs.port),
        Kind::Bucket => {
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
        Kind::Redis => Ok(()),
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

pub fn wait_for_postgres(container: &str, port: u16) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if pg_ready(container, port) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(Error::Engine(format!(
                "Postgres on 127.0.0.1:{port} (container {container}) did not accept connections within 60s"
            )));
        }
        thread::sleep(Duration::from_millis(400));
    }
}

fn pg_ready(container: &str, port: u16) -> bool {
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
    exec_ok || tcp_open(port)
}

pub fn tcp_open(port: u16) -> bool {
    TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_millis(200),
    )
    .is_ok()
}
