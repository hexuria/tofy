//! OpenTofu engine. `tofy apply` runs this when the spec backend is Tofu.
//! The user-facing command stays `tofy apply` / `tofy destroy`.

use std::path::Path;
use std::process::{Command, Stdio};

use tofy_spec::{replica_container, Project};

use crate::docker;
use crate::emit;
use crate::error::{Error, Result};
use crate::state::State;

pub fn available() -> bool {
    Command::new("tofu")
        .arg("version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn apply(root: &Path, spec: &Project, state: &State) -> Result<()> {
    emit::write_tofu_config(root, spec, state)?;
    let secrets = secret_values(state);
    run(root, &["init", "-input=false", "-no-color"], &secrets)?;
    run(
        root,
        &["apply", "-auto-approve", "-input=false", "-no-color"],
        &secrets,
    )?;
    wait_ready(spec, state)
}

pub fn destroy(root: &Path, state: &State) -> Result<()> {
    let spec = state.as_project();
    emit::write_tofu_config(root, &spec, state)?;
    let secrets = secret_values(state);
    run(root, &["init", "-input=false", "-no-color"], &secrets)?;
    run(
        root,
        &["destroy", "-auto-approve", "-input=false", "-no-color"],
        &secrets,
    )?;
    let _ = std::fs::remove_file(root.join(".tofy").join("main.tf.json"));
    Ok(())
}

fn wait_ready(spec: &Project, state: &State) -> Result<()> {
    for r in &spec.resources {
        let rs = state
            .resources
            .get(&r.name)
            .ok_or_else(|| Error::Engine(format!("missing prepared state for {}", r.name)))?;
        let name = replica_container(&spec.project, &r.name, 0);
        docker::ready_resource(r, rs, &name)?;
    }
    Ok(())
}

fn run(root: &Path, args: &[&str], secrets: &[String]) -> Result<()> {
    let dir = root.join(".tofy");
    std::fs::create_dir_all(&dir)?;
    let mut cmd = Command::new("tofu");
    cmd.current_dir(&dir);
    cmd.args(args);
    cmd.env("TF_IN_AUTOMATION", "1");
    cmd.env("TF_INPUT", "0");
    let out = cmd.output().map_err(|e| {
        Error::Engine(format!(
            "OpenTofu engine is required for this backend ({e})"
        ))
    })?;
    if !out.status.success() {
        let combined = format!(
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        return Err(Error::Engine(format!(
            "OpenTofu engine failed: {}",
            truncate(&redact(&combined, secrets), 4000)
        )));
    }
    Ok(())
}

fn secret_values(state: &State) -> Vec<String> {
    let mut out = Vec::new();
    for rs in state.resources.values() {
        for (key, value) in &rs.outputs {
            if tofy_spec::is_secret_key(key) && !value.is_empty() {
                out.push(value.clone());
            }
        }
    }
    out
}

fn redact(text: &str, secrets: &[String]) -> String {
    let mut s = text.to_string();
    for secret in secrets {
        if secret.len() >= 4 {
            s = s.replace(secret, "(redacted)");
        }
    }
    s
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_replaces_known_secrets() {
        let text = "password=supersecretvalue env=supersecretvalue";
        assert_eq!(
            redact(text, &["supersecretvalue".into()]),
            "password=(redacted) env=(redacted)"
        );
    }
}
