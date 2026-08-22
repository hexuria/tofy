//! Ambient AWS credentials and the AWS-provider OpenTofu path.
//!
//! `Backend::Aws` runs `tofu plan` / `tofu apply` / `tofu destroy` against an
//! emitted AWS-provider config. Credentials are read from the machine
//! (`AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY`, `AWS_PROFILE`, shared
//! config files). tofy does not mint, prompt, store, or commit credentials.

use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;
use tofy_spec::{Backend, Bind, Kind, Project};

use crate::emit;
use crate::error::{Error, Result};
use crate::state::State;
use crate::tofu;

/// True when the default credential chain already has something tofy can see:
/// access keys in the environment, `AWS_PROFILE`, or a shared credentials/config
/// file. Does not call AWS, prompt, or write anything.
pub fn credentials_available() -> bool {
    if env_nonempty("AWS_ACCESS_KEY_ID") && env_nonempty("AWS_SECRET_ACCESS_KEY") {
        return true;
    }
    if env_nonempty("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI")
        || env_nonempty("AWS_CONTAINER_CREDENTIALS_FULL_URI")
    {
        return true;
    }
    if env_nonempty("AWS_WEB_IDENTITY_TOKEN_FILE") {
        return true;
    }
    let profile = std::env::var("AWS_PROFILE").unwrap_or_else(|_| "default".into());
    let profile = profile.trim();
    if profile.is_empty() {
        return false;
    }
    profile_in_file(&credentials_file_path(), profile, false)
        || profile_in_file(&config_file_path(), profile, true)
}

/// `AWS_REGION` or `AWS_DEFAULT_REGION` when set.
pub fn region() -> Option<String> {
    for key in ["AWS_REGION", "AWS_DEFAULT_REGION"] {
        if let Ok(v) = std::env::var(key) {
            let v = v.trim();
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

fn env_nonempty(key: &str) -> bool {
    std::env::var(key)
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
}

fn credentials_file_path() -> PathBuf {
    if let Ok(p) = std::env::var("AWS_SHARED_CREDENTIALS_FILE") {
        if !p.trim().is_empty() {
            return PathBuf::from(p);
        }
    }
    home_aws().join("credentials")
}

fn config_file_path() -> PathBuf {
    if let Ok(p) = std::env::var("AWS_CONFIG_FILE") {
        if !p.trim().is_empty() {
            return PathBuf::from(p);
        }
    }
    home_aws().join("config")
}

fn home_aws() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".aws")
}

fn profile_in_file(path: &Path, profile: &str, config_style: bool) -> bool {
    let Ok(text) = fs::read_to_string(path) else {
        return false;
    };
    let headers = profile_headers(profile, config_style);
    text.lines().any(|line| {
        let line = line.trim();
        headers.iter().any(|h| line.eq_ignore_ascii_case(h))
    })
}

fn profile_headers(profile: &str, config_style: bool) -> Vec<String> {
    let mut out = vec![format!("[{profile}]")];
    if config_style && profile != "default" {
        out.push(format!("[profile {profile}]"));
    }
    out
}

pub fn apply(root: &Path, spec: &Project, state: &mut State) -> Result<()> {
    emit::write_tofu_config(root, spec, state)?;
    let secrets = tofu::secret_values(state);
    tofu::run(root, &["init", "-input=false", "-no-color"], &secrets)?;
    tofu::run(
        root,
        &["apply", "-auto-approve", "-input=false", "-no-color"],
        &secrets,
    )?;
    refresh_outputs(root, spec, state, &secrets)
}

/// Emit `.tofy/main.tf.json` (0600), `tofu init` if needed, and return `tofu plan`.
/// Does not persist Applied status. Missing tofu or missing ambient AWS
/// credentials is an error, not "No changes."
pub fn plan(root: &Path, spec: &Project, state: &State) -> Result<String> {
    let mut emit_state = state.clone();
    emit::write_tofu_config(root, spec, &mut emit_state)?;
    if !tofu::available() {
        return Err(Error::PlanNeedsTofu);
    }
    if !credentials_available() {
        return Err(Error::PlanNeedsAwsCredentials);
    }
    let secrets = tofu::secret_values(&emit_state);
    tofu::run(root, &["init", "-input=false", "-no-color"], &secrets)?;
    tofu::run_output(root, &["plan", "-input=false", "-no-color"], &secrets)
}

pub fn destroy(root: &Path, state: &State) -> Result<()> {
    let spec = state.as_project();
    let mut emit_state = state.clone();
    emit::write_tofu_config_mode(root, &spec, &mut emit_state, CidrMode::PreferPersisted)?;
    let secrets = tofu::secret_values(&emit_state);
    tofu::run(root, &["init", "-input=false", "-no-color"], &secrets)?;
    tofu::run(
        root,
        &["destroy", "-auto-approve", "-input=false", "-no-color"],
        &secrets,
    )?;
    let _ = std::fs::remove_file(root.join(".tofy").join("main.tf.json"));
    Ok(())
}

fn refresh_outputs(
    root: &Path,
    spec: &Project,
    state: &mut State,
    secrets: &[String],
) -> Result<()> {
    let raw = tofu::run_output(root, &["output", "-json", "-no-color"], secrets)?;
    let parsed: Value = serde_json::from_str(raw.trim())
        .map_err(|e| Error::Engine(format!("OpenTofu engine output was not JSON: {e}")))?;
    merge_engine_outputs(&parsed, spec, state)
}

/// ElastiCache is emitted with transit encryption and AUTH. Clients must use TLS.
pub fn redis_uri(password: &str, host: &str, port: impl std::fmt::Display) -> String {
    format!("rediss://:{password}@{host}:{port}")
}

fn merge_engine_outputs(parsed: &Value, spec: &Project, state: &mut State) -> Result<()> {
    for r in &spec.resources {
        let rs = state
            .resources
            .get_mut(&r.name)
            .ok_or_else(|| Error::Engine(format!("missing prepared state for {}", r.name)))?;
        match r.kind {
            Kind::Postgres | Kind::Mysql => {
                let host = output_string(parsed, &format!("{}_host", r.name))?;
                let port = output_string(parsed, &format!("{}_port", r.name))
                    .unwrap_or_else(|_| rs.port.to_string());
                let user = rs
                    .outputs
                    .get("user")
                    .cloned()
                    .unwrap_or_else(|| "tofy".into());
                let password = rs.outputs.get("password").cloned().unwrap_or_default();
                let database = rs
                    .outputs
                    .get("database")
                    .cloned()
                    .unwrap_or_else(|| r.name.replace('-', "_"));
                let scheme = match r.kind {
                    Kind::Mysql => "mysql",
                    _ => "postgres",
                };
                rs.port = port.parse().unwrap_or(rs.port);
                rs.outputs.insert("host".into(), host.clone());
                rs.outputs.insert("port".into(), port.clone());
                rs.outputs.insert(
                    "uri".into(),
                    format!("{scheme}://{user}:{password}@{host}:{port}/{database}"),
                );
            }
            Kind::Redis => {
                let host = output_string(parsed, &format!("{}_host", r.name))?;
                let port = output_string(parsed, &format!("{}_port", r.name))
                    .unwrap_or_else(|_| rs.port.to_string());
                let password = rs.outputs.get("password").cloned().unwrap_or_default();
                rs.port = port.parse().unwrap_or(rs.port);
                rs.outputs.insert("host".into(), host.clone());
                rs.outputs.insert("port".into(), port.clone());
                rs.outputs
                    .insert("uri".into(), redis_uri(&password, &host, &port));
            }
            Kind::Bucket => {
                let bucket = output_string(parsed, &format!("{}_bucket", r.name))?;
                let region = output_string(parsed, &format!("{}_region", r.name))?;
                let endpoint = output_string(parsed, &format!("{}_endpoint", r.name))?;
                rs.outputs.insert("bucket".into(), bucket);
                rs.outputs.insert("region".into(), region);
                rs.outputs.insert("endpoint".into(), endpoint);
                rs.outputs.remove("access_key");
                rs.outputs.remove("secret_key");
            }
            Kind::Secret => {}
        }
    }
    Ok(())
}

fn output_string(parsed: &Value, name: &str) -> Result<String> {
    let value = parsed
        .get(name)
        .and_then(|o| o.get("value"))
        .or_else(|| parsed.get(name));
    match value {
        Some(Value::String(s)) if !s.is_empty() => Ok(s.clone()),
        Some(Value::Number(n)) => Ok(n.to_string()),
        _ => Err(Error::Engine(format!(
            "OpenTofu engine did not return output {name}"
        ))),
    }
}

/// DNS-safe token for S3 / RDS / ElastiCache identifiers.
pub fn aws_token(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "tofy".into()
    } else {
        out
    }
}

pub fn resource_id(project: &str, name: &str) -> String {
    format!("tofy-{}-{}", aws_token(project), aws_token(name))
}

pub fn s3_bucket_name(project: &str, name: &str, suffix: &str) -> String {
    let mut bucket = format!(
        "tofy-{}-{}-{}",
        aws_token(project),
        aws_token(name),
        aws_token(suffix)
    );
    if bucket.len() > 63 {
        bucket.truncate(63);
        bucket = bucket.trim_end_matches('-').to_string();
    }
    bucket
}

pub fn expected_backend(spec: &Project) -> Result<()> {
    if spec.backend != Backend::Aws {
        return Err(Error::Engine(
            "AWS OpenTofu path requires backend aws".into(),
        ));
    }
    Ok(())
}

/// How to resolve the applier `/32` before emitting AWS JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CidrMode {
    /// Plan / apply / emit: always rediscover so an IP change is a plan update.
    Rediscover,
    /// Destroy: keep the persisted `/32` so the config stays stable offline.
    PreferPersisted,
}

pub(crate) fn needs_engine_sg(spec: &Project) -> bool {
    spec.resources
        .iter()
        .any(|r| matches!(r.kind, Kind::Postgres | Kind::Mysql | Kind::Redis))
}

pub(crate) fn needs_applier_cidr(spec: &Project) -> bool {
    spec.resources.iter().any(|r| {
        matches!(r.kind, Kind::Postgres | Kind::Mysql | Kind::Redis) && r.bind == Bind::Localhost
    })
}

pub(crate) fn prepare_emit(spec: &Project, state: &mut State, mode: CidrMode) -> Result<()> {
    if spec.backend != Backend::Aws || !needs_engine_sg(spec) {
        return Ok(());
    }
    if !needs_applier_cidr(spec) {
        return Ok(());
    }
    match mode {
        CidrMode::Rediscover => {
            state.applier_cidr = Some(discover_applier_cidr()?);
        }
        CidrMode::PreferPersisted => {
            if state.applier_cidr.is_none() {
                state.applier_cidr = Some(discover_applier_cidr()?);
            }
        }
    }
    Ok(())
}

/// Public IPv4 of this machine as `a.b.c.d/32`. Never returns `0.0.0.0/0`.
pub fn discover_applier_cidr() -> Result<String> {
    #[cfg(test)]
    if let Some(stub) = test_stub() {
        return match stub {
            Ok(raw) => parse_applier_cidr(&raw),
            Err(()) => Err(Error::PublicIpv4Undetermined),
        };
    }
    if let Ok(raw) = std::env::var("TOFY_APPLIER_CIDR") {
        let raw = raw.trim();
        if !raw.is_empty() {
            return parse_applier_cidr(raw);
        }
    }
    match fetch_public_ipv4() {
        Some(ip) => parse_applier_cidr(&ip.to_string()),
        None => Err(Error::PublicIpv4Undetermined),
    }
}

fn parse_applier_cidr(raw: &str) -> Result<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw == "0.0.0.0/0" || raw == "0.0.0.0" {
        return Err(Error::PublicIpv4Undetermined);
    }
    let ip_part = if let Some(ip) = raw.strip_suffix("/32") {
        ip
    } else if raw.contains('/') {
        return Err(Error::PublicIpv4Undetermined);
    } else {
        raw
    };
    let ip: Ipv4Addr = ip_part.parse().map_err(|_| Error::PublicIpv4Undetermined)?;
    if ip.is_unspecified() || ip.is_loopback() || ip.is_multicast() || ip.is_broadcast() {
        return Err(Error::PublicIpv4Undetermined);
    }
    Ok(format!("{ip}/32"))
}

fn fetch_public_ipv4() -> Option<Ipv4Addr> {
    const TARGETS: &[(&str, &str)] = &[
        ("checkip.amazonaws.com", "/"),
        ("icanhazip.com", "/"),
        ("ifconfig.me", "/ip"),
    ];
    for (host, path) in TARGETS {
        if let Some(ip) = http_ipv4(host, path) {
            return Some(ip);
        }
    }
    None
}

fn http_ipv4(host: &str, path: &str) -> Option<Ipv4Addr> {
    let addr = (host, 80).to_socket_addrs().ok()?.find(|a| a.is_ipv4())?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .ok()?;
    let req = format!(
        "GET {path} HTTP/1.0\r\nHost: {host}\r\nUser-Agent: tofy\r\nAccept: text/plain\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).ok()?;
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let body = text.split("\r\n\r\n").nth(1).unwrap_or(&text);
    let token = body
        .chars()
        .filter(|c| !c.is_whitespace())
        .take(32)
        .collect::<String>();
    let ip: Ipv4Addr = token.parse().ok()?;
    if ip.is_unspecified() || ip.is_loopback() || ip.is_multicast() || ip.is_broadcast() {
        return None;
    }
    Some(ip)
}

#[cfg(test)]
thread_local! {
    static IP_STUB: std::cell::RefCell<Option<std::result::Result<String, ()>>> =
        std::cell::RefCell::new(None);
}

#[cfg(test)]
fn test_stub() -> Option<std::result::Result<String, ()>> {
    IP_STUB.with(|c| c.borrow().clone())
}

#[cfg(test)]
static DISCOVER_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Pin public-IP discovery for unit tests / CI. Not a product networking knob.
#[cfg(test)]
pub(crate) fn with_applier_cidr<T>(cidr: &str, f: impl FnOnce() -> T) -> T {
    let _g = DISCOVER_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    IP_STUB.with(|c| *c.borrow_mut() = Some(Ok(cidr.to_string())));
    let out = f();
    IP_STUB.with(|c| *c.borrow_mut() = None);
    out
}

#[cfg(test)]
pub(crate) fn with_public_ip_undetermined<T>(f: impl FnOnce() -> T) -> T {
    let _g = DISCOVER_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    IP_STUB.with(|c| *c.borrow_mut() = Some(Err(())));
    let out = f();
    IP_STUB.with(|c| *c.borrow_mut() = None);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_dns_safe() {
        assert_eq!(aws_token("Demo_AWS"), "demo-aws");
        assert_eq!(resource_id("demoaws", "appdb"), "tofy-demoaws-appdb");
        let bucket = s3_bucket_name("demoaws", "uploads", "AbC123xy");
        assert!(bucket.starts_with("tofy-demoaws-uploads-"));
        assert!(bucket
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
        assert!(bucket.len() <= 63);
    }

    #[test]
    fn profile_headers_cover_config_style() {
        assert_eq!(
            profile_headers("default", true),
            vec!["[default]".to_string()]
        );
        assert!(profile_headers("work", true).contains(&"[profile work]".into()));
        assert!(profile_headers("work", false).contains(&"[work]".into()));
    }

    #[test]
    fn redis_uri_is_tls_because_elasticache_enables_transit_encryption() {
        let uri = redis_uri("s3cretValue", "cache.example.cache.amazonaws.com", 6379);
        assert!(uri.starts_with("rediss://:"), "{uri}");
        assert!(!uri.starts_with("redis://:"), "{uri}");
        assert_eq!(
            uri,
            "rediss://:s3cretValue@cache.example.cache.amazonaws.com:6379"
        );
    }

    #[test]
    fn merge_engine_outputs_writes_rediss_uri() {
        use crate::state::prepare_state;
        use tofy_spec::{Kind, Project, Resource};

        let mut spec = Project::new("demoaws");
        spec.backend = Backend::Aws;
        spec.resources
            .push(Resource::new("cache", Kind::Redis).with_port(26379));
        let mut state = prepare_state(&spec, &State::default());
        let password = state.resources["cache"].outputs["password"].clone();
        let parsed = serde_json::json!({
            "cache_host": { "value": "master.demoaws.cache.amazonaws.com" },
            "cache_port": { "value": 26379 }
        });
        merge_engine_outputs(&parsed, &spec, &mut state).unwrap();
        let uri = &state.resources["cache"].outputs["uri"];
        assert_eq!(
            uri,
            &format!("rediss://:{password}@master.demoaws.cache.amazonaws.com:26379")
        );
        assert!(uri.starts_with("rediss://:"), "{uri}");
        assert!(!uri.contains("redis://:"), "{uri}");
    }

    #[test]
    fn parse_applier_cidr_is_slash32_only() {
        assert_eq!(
            parse_applier_cidr("203.0.113.10").unwrap(),
            "203.0.113.10/32"
        );
        assert_eq!(
            parse_applier_cidr("203.0.113.10/32").unwrap(),
            "203.0.113.10/32"
        );
        assert!(parse_applier_cidr("0.0.0.0/0").is_err());
        assert!(parse_applier_cidr("0.0.0.0").is_err());
        assert!(parse_applier_cidr("127.0.0.1").is_err());
        assert!(parse_applier_cidr("203.0.113.10/24").is_err());
        assert!(parse_applier_cidr("not-an-ip").is_err());
    }

    #[test]
    fn discover_uses_stub_and_missing_is_an_error() {
        with_applier_cidr("203.0.113.10/32", || {
            assert_eq!(discover_applier_cidr().unwrap(), "203.0.113.10/32");
        });
        with_public_ip_undetermined(|| {
            assert!(matches!(
                discover_applier_cidr(),
                Err(Error::PublicIpv4Undetermined)
            ));
        });
    }
}
