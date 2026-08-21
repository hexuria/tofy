use std::path::Path;
use std::process::Command;

use crate::spec::Project;
use crate::state::{self, Action, State, Status};

pub fn apply(root: &Path, spec: &Project) -> Result<String, crate::Error> {
    let current = State::load(root)?;
    let actions = state::plan(spec, &current);
    crate::emit::write_artifacts(root, spec)?;

    let mut next = State::from_spec(spec);
    let docker = docker_available();
    if docker {
        run_compose(root, &["up", "-d"])?;
        for r in next.resources.values_mut() {
            r.status = Status::Applied;
        }
    } else {
        for r in next.resources.values_mut() {
            r.status = Status::Emitted;
        }
    }
    next.save(root)?;

    let mut msg = String::new();
    msg.push_str(&format_actions(&actions));
    msg.push('\n');
    if docker {
        msg.push_str("Applied with docker compose. Outputs in .tofy/outputs.json\n");
    } else {
        msg.push_str("Docker is not on this machine. Wrote .tofy/main.tf.json and docker-compose.yml.\n");
        msg.push_str("On a machine with Docker: docker compose up -d\n");
        msg.push_str("On a machine with OpenTofu: cd .tofy && tofu init && tofu apply\n");
    }
    Ok(msg)
}

pub fn destroy(root: &Path) -> Result<String, crate::Error> {
    let mut current = State::load(root)?;
    if current.resources.is_empty() {
        return Ok("Nothing in state.\n".into());
    }
    if docker_available() && root.join("docker-compose.yml").exists() {
        let _ = run_compose(root, &["down", "-v"]);
    }
    current.resources.clear();
    current.save(root)?;
    Ok("Destroyed local resources and cleared state. Terraform/OpenTofu state in .tofy is yours to destroy with tofu destroy.\n".into())
}

pub fn format_actions(actions: &[Action]) -> String {
    if actions.is_empty() {
        return "No resources in spec.\n".into();
    }
    let mut s = String::from("Plan:\n");
    for a in actions {
        match a {
            Action::Create { name, kind } => s.push_str(&format!("  + create  {name}  ({kind:?})\n")),
            Action::Update { name, reason } => s.push_str(&format!("  ~ update  {name}  ({reason})\n")),
            Action::Delete { name, kind } => s.push_str(&format!("  - delete  {name}  ({kind:?})\n")),
            Action::Noop { name } => s.push_str(&format!("    noop    {name}\n")),
        }
    }
    s
}

fn docker_available() -> bool {
    Command::new("docker")
        .args(["compose", "version"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn run_compose(root: &Path, args: &[&str]) -> Result<(), crate::Error> {
    let mut cmd = Command::new("docker");
    cmd.arg("compose").args(args).current_dir(root);
    let out = cmd.output()?;
    if !out.status.success() {
        return Err(crate::Error::Engine(format!(
            "docker compose failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(())
}
