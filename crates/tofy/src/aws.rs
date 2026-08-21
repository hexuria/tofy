//! Ambient AWS credentials and the AWS-provider OpenTofu path.
//!
//! `Backend::Aws` runs `tofu plan` / `tofu apply` / `tofu destroy` against an
//! emitted AWS-provider config. Credentials are read from the machine
//! (`AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY`, `AWS_PROFILE`, shared
//! config files). tofy does not mint, prompt, store, or commit credentials.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use tofy_spec::{Backend, Kind, Project};

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
    std::env::var(key).map(|v| !v.trim().is_empty()).unwrap_or(false)
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
    emit::write_tofu_config(root, spec, state)?;
    if !tofu::available() {
        return Err(Error::PlanNeedsTofu);
    }
    if !credentials_available() {
        return Err(Error::PlanNeedsAwsCredentials);
    }
    let secrets = tofu::secret_values(state);
    tofu::run(root, &["init", "-input=false", "-no-color"], &secrets)?;
    tofu::run_output(root, &["plan", "-input=false", "-no-color"], &secrets)
}

pub fn destroy(root: &Path, state: &State) -> Result<()> {
    let spec = state.as_project();
    emit::write_tofu_config(root, &spec, state)?;
    let secrets = tofu::secret_values(state);
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
    let parsed: Value = serde_json::from_str(raw.trim()).map_err(|e| {
        Error::Engine(format!("OpenTofu engine output was not JSON: {e}"))
    })?;
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
            Kind::Postgres => {
                let host = output_string(parsed, &format!("{}_host", r.name))?;
                let port = output_string(parsed, &format!("{}_port", r.name))
                    .unwrap_or_else(|_| rs.port.to_string());
                let user = rs.outputs.get("user").cloned().unwrap_or_else(|| "tofy".into());
                let password = rs.outputs.get("password").cloned().unwrap_or_default();
                let database = rs
                    .outputs
                    .get("database")
                    .cloned()
                    .unwrap_or_else(|| r.name.replace('-', "_"));
                rs.port = port.parse().unwrap_or(rs.port);
                rs.outputs.insert("host".into(), host.clone());
                rs.outputs.insert("port".into(), port.clone());
                rs.outputs.insert(
                    "uri".into(),
                    format!("postgres://{user}:{password}@{host}:{port}/{database}"),
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
                rs.outputs.insert("uri".into(), redis_uri(&password, &host, &port));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_dns_safe() {
        assert_eq!(aws_token("Demo_AWS"), "demo-aws");
        assert_eq!(resource_id("demoaws", "appdb"), "tofy-demoaws-appdb");
        let bucket = s3_bucket_name("demoaws", "uploads", "AbC123xy");
        assert!(bucket.starts_with("tofy-demoaws-uploads-"));
        assert!(bucket.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
        assert!(bucket.len() <= 63);
    }

    #[test]
    fn profile_headers_cover_config_style() {
        assert_eq!(profile_headers("default", true), vec!["[default]".to_string()]);
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
        spec.resources.push(Resource::new("cache", Kind::Redis).with_port(26379));
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
}
