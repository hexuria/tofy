use std::path::Path;

use tofy_spec::{Kind, Project};

use crate::docker;
use crate::emit;
use crate::error::{Error, Result};
use crate::lock::Lock;
use crate::outputs;
use crate::state::{self, prepare_state, State};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Create { name: String, kind: Kind },
    Update { name: String, reason: String },
    Delete { name: String, kind: Kind },
    Noop { name: String },
}

impl Action {
    pub fn is_change(&self) -> bool {
        !matches!(self, Action::Noop { .. })
    }
}

pub fn plan(spec: &Project, current: &State) -> Vec<Action> {
    let desired = prepare_state(spec, current);
    let mut actions = Vec::new();
    for (name, want) in &desired.resources {
        match current.resources.get(name) {
            None => actions.push(Action::Create {
                name: name.clone(),
                kind: want.kind,
            }),
            Some(have)
                if have.kind != want.kind
                    || have.image != want.image
                    || have.port != want.port
                    || have.size != want.size
                    || have.bind != want.bind
                    || have.replicas != want.replicas =>
            {
                let mut parts = Vec::new();
                if have.kind != want.kind {
                    parts.push("type");
                }
                if have.image != want.image {
                    parts.push("image");
                }
                if have.port != want.port {
                    parts.push("port");
                }
                if have.size != want.size {
                    parts.push("size");
                }
                if have.bind != want.bind {
                    parts.push("bind");
                }
                if have.replicas != want.replicas {
                    parts.push("replicas");
                }
                actions.push(Action::Update {
                    name: name.clone(),
                    reason: parts.join(", ") + " changed",
                });
            }
            Some(_) => actions.push(Action::Noop { name: name.clone() }),
        }
    }
    for (name, have) in &current.resources {
        if !desired.resources.contains_key(name) {
            actions.push(Action::Delete {
                name: name.clone(),
                kind: have.kind,
            });
        }
    }
    actions
}

pub fn format_actions(actions: &[Action]) -> String {
    if actions.is_empty() {
        return "No resources.\n".into();
    }
    if actions.iter().all(|a| !a.is_change()) {
        return "No changes.\n".into();
    }
    let mut s = String::from("Plan:\n");
    for a in actions {
        match a {
            Action::Create { name, kind } => {
                s.push_str(&format!("  + create  {name}  ({kind})\n"));
            }
            Action::Update { name, reason } => {
                s.push_str(&format!("  ~ update  {name}  ({reason})\n"));
            }
            Action::Delete { name, kind } => {
                s.push_str(&format!("  - delete  {name}  ({kind})\n"));
            }
            Action::Noop { name } => {
                s.push_str(&format!("    noop    {name}\n"));
            }
        }
    }
    s
}

pub fn plan_text(root: &Path, spec: &Project) -> Result<String> {
    spec.validate()?;
    let current = State::load(root)?;
    Ok(format_actions(&plan(spec, &current)))
}

pub fn apply(root: &Path, spec: &Project) -> Result<String> {
    spec.validate()?;
    let _lock = Lock::acquire(root)?;
    let current = State::load(root)?;
    let actions = plan(spec, &current);
    let mut next = prepare_state(spec, &current);
    emit::write_artifacts(root, spec, &next)?;

    if !docker::available() {
        state::mark_emitted(&mut next);
        next.save(root)?;
        outputs::write(root, &next)?;
        print!("{}", format_actions(&actions));
        println!();
        return Err(Error::DockerMissing);
    }

    if spec.resources.is_empty() {
        docker::destroy_network(&spec.project)?;
    } else {
        docker::ensure_network(&spec.project)?;
    }

    // Deletes first so ports can be reused.
    for a in &actions {
        if let Action::Delete { name, .. } = a {
            let n = current.resources.get(name).map(|r| r.replicas).unwrap_or(1);
            docker::destroy_resource(&current.project, name, n)?;
        }
    }

    for r in &spec.resources {
        let rs = next
            .resources
            .get(&r.name)
            .ok_or_else(|| Error::Engine(format!("missing prepared state for {}", r.name)))?;
        let action = actions.iter().find(|a| match a {
            Action::Create { name, .. }
            | Action::Update { name, .. }
            | Action::Noop { name }
            | Action::Delete { name, .. } => name == &r.name,
        });
        match action {
            Some(Action::Create { .. }) | Some(Action::Update { .. }) => {
                docker::start_resource(spec, r, rs)?;
            }
            Some(Action::Noop { .. }) | None => {
                docker::ensure_running(spec, r, rs)?;
            }
            Some(Action::Delete { .. }) => {}
        }
    }

    state::mark_applied(&mut next);
    next.save(root)?;
    outputs::write(root, &next)?;

    let mut msg = format_actions(&actions);
    msg.push('\n');
    msg.push_str("Applied. Outputs written to .tofy/outputs.env\n");
    Ok(msg)
}

pub fn destroy(root: &Path) -> Result<String> {
    let _lock = Lock::acquire(root)?;
    let mut current = State::load(root)?;
    if current.resources.is_empty() {
        outputs::clear(root)?;
        return Ok("Nothing in state.\n".into());
    }
    if docker::available() {
        let project = current.project.clone();
        for (name, rs) in &current.resources {
            docker::destroy_resource(&project, name, rs.replicas)?;
        }
        docker::destroy_network(&project)?;
    }
    current.clear_resources();
    current.save(root)?;
    outputs::clear(root)?;
    Ok("Destroyed local resources and cleared state.\n".into())
}

pub fn emit(root: &Path, spec: &Project) -> Result<String> {
    spec.validate()?;
    let current = State::load(root)?;
    let next = prepare_state(spec, &current);
    emit::write_artifacts(root, spec, &next)?;
    Ok("Wrote .tofy/spec.json and .tofy/main.tf.json\n".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::State;
    use tofy_spec::{Kind, Resource};

    fn spec(resources: &[(&str, Kind, Option<u16>)]) -> Project {
        let mut p = Project::new("demo");
        for (name, kind, port) in resources {
            let mut r = Resource::new(*name, *kind);
            r.port = *port;
            p.resources.push(r);
        }
        p
    }

    fn state_from(spec: &Project) -> State {
        prepare_state(spec, &State::default())
    }

    #[test]
    fn plan_three_creates() {
        let spec = spec(&[
            ("appdb", Kind::Postgres, Some(5433)),
            ("cache", Kind::Redis, None),
            ("uploads", Kind::Bucket, None),
        ]);
        let actions = plan(&spec, &State::default());
        let creates: Vec<_> = actions
            .iter()
            .filter_map(|a| match a {
                Action::Create { name, kind } => Some((name.as_str(), *kind)),
                _ => None,
            })
            .collect();
        assert_eq!(
            creates,
            vec![
                ("appdb", Kind::Postgres),
                ("cache", Kind::Redis),
                ("uploads", Kind::Bucket),
            ]
        );
        let text = format_actions(&actions);
        assert!(text.contains("+ create  appdb  (postgres)"));
        assert!(text.contains("+ create  cache  (redis)"));
        assert!(text.contains("+ create  uploads  (bucket)"));
        assert!(!text.to_lowercase().contains("password"));
    }

    #[test]
    fn plan_update_and_delete() {
        let current_spec = spec(&[
            ("appdb", Kind::Postgres, Some(5433)),
            ("cache", Kind::Redis, None),
        ]);
        let current = state_from(&current_spec);
        let desired = spec(&[
            ("appdb", Kind::Postgres, Some(5434)),
            ("uploads", Kind::Bucket, None),
        ]);
        let actions = plan(&desired, &current);
        assert!(actions.iter().any(|a| matches!(
            a,
            Action::Update { name, .. } if name == "appdb"
        )));
        assert!(actions
            .iter()
            .any(|a| matches!(a, Action::Create { name, .. } if name == "uploads")));
        assert!(actions
            .iter()
            .any(|a| matches!(a, Action::Delete { name, .. } if name == "cache")));
        let text = format_actions(&actions);
        assert!(text.contains("~ update  appdb"));
        assert!(text.contains("port changed"));
        assert!(!text.to_lowercase().contains("password"));
    }

    #[test]
    fn plan_noop_when_unchanged() {
        let spec = spec(&[("cache", Kind::Redis, None)]);
        let current = state_from(&spec);
        let actions = plan(&spec, &current);
        assert!(actions.iter().all(|a| matches!(a, Action::Noop { .. })));
        assert_eq!(format_actions(&actions), "No changes.\n");
    }

    #[test]
    fn prepare_keeps_password_across_plan() {
        let spec = spec(&[("appdb", Kind::Postgres, Some(5433))]);
        let first = prepare_state(&spec, &State::default());
        let pass = first.resources["appdb"].outputs["password"].clone();
        let second = prepare_state(&spec, &first);
        assert_eq!(pass, second.resources["appdb"].outputs["password"]);
        let actions = plan(&spec, &first);
        assert!(actions.iter().all(|a| matches!(a, Action::Noop { .. })));
    }

    #[test]
    fn plan_shows_size_update() {
        let current_spec = spec(&[("cache", Kind::Redis, None)]);
        let current = state_from(&current_spec);
        let mut desired = spec(&[("cache", Kind::Redis, None)]);
        desired.resources[0].size = tofy_spec::Size::Large;
        let actions = plan(&desired, &current);
        let text = format_actions(&actions);
        assert!(text.contains("~ update  cache"), "{text}");
        assert!(text.contains("size"), "{text}");
        assert!(!text.to_lowercase().contains("password"));
    }

    #[test]
    fn plan_does_not_mark_resources_applied() {
        let dir = tempfile::tempdir().unwrap();
        let spec = spec(&[
            ("appdb", Kind::Postgres, Some(5433)),
            ("cache", Kind::Redis, None),
            ("uploads", Kind::Bucket, None),
        ]);
        let text = plan_text(dir.path(), &spec).unwrap();
        assert!(text.contains("+ create"), "{text}");
        let current = State::load(dir.path()).unwrap();
        assert!(
            current
                .resources
                .values()
                .all(|r| r.status != crate::state::Status::Applied),
            "plan must not mark resources Applied"
        );
        assert!(!dir.path().join(".tofy").join("outputs.env").exists());
    }

    #[test]
    fn plan_shows_bind_update() {
        let current_spec = spec(&[("appdb", Kind::Postgres, Some(5433))]);
        let current = state_from(&current_spec);
        let mut desired = spec(&[("appdb", Kind::Postgres, Some(5433))]);
        desired.resources[0].bind = tofy_spec::Bind::All;
        let text = format_actions(&plan(&desired, &current));
        assert!(text.contains("bind changed"), "{text}");
    }

    #[test]
    fn apply_without_docker_emits_and_errors() {
        if docker::available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let spec = spec(&[
            ("appdb", Kind::Postgres, Some(5433)),
            ("cache", Kind::Redis, None),
            ("uploads", Kind::Bucket, None),
        ]);
        let err = apply(dir.path(), &spec).unwrap_err();
        assert!(matches!(err, Error::DockerMissing), "{err}");
        assert!(!err.to_string().contains("Applied"));
        assert!(dir.path().join(".tofy").join("spec.json").exists());
        assert!(dir.path().join(".tofy").join("main.tf.json").exists());
        let first = outputs::load(dir.path()).unwrap();
        let again = apply(dir.path(), &spec).unwrap_err();
        assert!(matches!(again, Error::DockerMissing));
        let second = outputs::load(dir.path()).unwrap();
        assert_eq!(first["TOFY_APPDB_PASSWORD"], second["TOFY_APPDB_PASSWORD"]);
        assert!(!first["TOFY_APPDB_PASSWORD"].starts_with("tofy-"));
    }
}
