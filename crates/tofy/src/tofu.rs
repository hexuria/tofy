//! OpenTofu engine. `tofy apply` / `tofy plan` / `tofy destroy` run this when
//! the spec backend is Tofu. The user-facing commands stay `tofy apply` /
//! `tofy plan` / `tofy destroy` — never "go run tofu …".

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
    let mut emit_state = state.clone();
    emit::write_tofu_config(root, spec, &mut emit_state)?;
    let secrets = secret_values(state);
    run(root, &["init", "-input=false", "-no-color"], &secrets)?;
    run(
        root,
        &["apply", "-auto-approve", "-input=false", "-no-color"],
        &secrets,
    )?;
    wait_ready(spec, state)
}

/// Emit `.tofy/main.tf.json` (0600), `tofu init` if needed, and return `tofu plan`.
/// Does not persist Applied status. Missing tofu is an error, not "No changes."
pub fn plan(root: &Path, spec: &Project, state: &State) -> Result<String> {
    let mut emit_state = state.clone();
    emit::write_tofu_config(root, spec, &mut emit_state)?;
    if !available() {
        return Err(Error::PlanNeedsTofu);
    }
    let secrets = secret_values(state);
    run(root, &["init", "-input=false", "-no-color"], &secrets)?;
    run_output(root, &["plan", "-input=false", "-no-color"], &secrets)
}

pub fn destroy(root: &Path, state: &State) -> Result<()> {
    let spec = state.as_project();
    let mut emit_state = state.clone();
    emit::write_tofu_config(root, &spec, &mut emit_state)?;
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
        if !r.kind.is_runtime() {
            continue;
        }
        let rs = state
            .resources
            .get(&r.name)
            .ok_or_else(|| Error::Engine(format!("missing prepared state for {}", r.name)))?;
        let n = r.replicas_or_default();
        for i in 0..n {
            let name = replica_container(&spec.project, &r.name, i);
            docker::ready_replica(r, rs, &name, i)?;
        }
    }
    Ok(())
}

pub(crate) fn run(root: &Path, args: &[&str], secrets: &[String]) -> Result<()> {
    run_output(root, args, secrets).map(|_| ())
}

pub(crate) fn run_output(root: &Path, args: &[&str], secrets: &[String]) -> Result<String> {
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
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let redacted = redact(&combined, secrets);
    if !out.status.success() {
        return Err(Error::Engine(format!(
            "OpenTofu engine failed: {}",
            truncate(&redacted, 4000)
        )));
    }
    Ok(redacted)
}

pub(crate) fn secret_values(state: &State) -> Vec<String> {
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
