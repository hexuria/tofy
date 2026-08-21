//! Optional Docker smoke test. Skips when Docker is not available.

use std::path::Path;

use tofy::docker;
use tofy::engine;
use tofy::outputs;
use tofy::spec::{Backend, Kind, Project, Resource};
use tofy::tofu;

fn demo_spec() -> Project {
    let mut p = Project::new("tofy-smoke");
    p.resources.push(
        Resource::new("appdb", Kind::Postgres)
            .with_version("16")
            .with_port(55433)
            .with_size(tofy::spec::Size::Small),
    );
    p.resources
        .push(Resource::new("cache", Kind::Redis).with_port(56379));
    p.resources
        .push(Resource::new("uploads", Kind::Bucket).with_port(59000));
    p
}

#[test]
fn smoke_apply_example_if_docker() {
    if !docker::available() {
        eprintln!("skipping smoke apply: Docker is not available");
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let spec = demo_spec();

    let first = engine::apply(root, &spec).expect("apply");
    assert!(first.contains("+ create  appdb"), "{first}");
    assert!(first.contains("+ create  cache"), "{first}");
    assert!(first.contains("+ create  uploads"), "{first}");
    assert!(first.contains("Applied."), "{first}");

    let outs = outputs::load(root).unwrap();
    assert!(outs.contains_key("TOFY_APPDB_URI"));
    assert!(outs.contains_key("TOFY_APPDB_PASSWORD"));
    assert!(outs.contains_key("TOFY_CACHE_URI"));
    assert!(outs.contains_key("TOFY_CACHE_PASSWORD"));
    assert!(outs["TOFY_CACHE_URI"].contains(&outs["TOFY_CACHE_PASSWORD"]));
    assert!(outs.contains_key("TOFY_UPLOADS_SECRET_KEY"));
    assert_eq!(
        outs.get("TOFY_UPLOADS_BUCKET").map(String::as_str),
        Some("uploads")
    );
    let password = outs["TOFY_APPDB_PASSWORD"].clone();
    assert_ne!(password, "tofy-tofy-smoke-appdb");
    assert!(!password.starts_with("tofy-"));

    assert!(root.join(".tofy").join("state.json").exists());
    assert!(root.join(".tofy").join("outputs.env").exists());
    assert!(root.join(".tofy").join("spec.json").exists());

    let second = engine::apply(root, &spec).expect("second apply");
    assert!(
        second.contains("No changes.") || !second.contains("+ create"),
        "{second}"
    );
    let outs2 = outputs::load(root).unwrap();
    assert_eq!(password, outs2["TOFY_APPDB_PASSWORD"]);

    let cache = tofy::spec::replica_container(&spec.project, "cache", 0);
    assert!(
        std::process::Command::new("docker")
            .args(["stop", &cache])
            .status()
            .unwrap()
            .success(),
        "docker stop {cache}"
    );
    let drifted = engine::plan_text(root, &spec).expect("plan after stop");
    assert!(!drifted.contains("No changes."), "{drifted}");
    assert!(
        drifted.contains("not running")
            || drifted.contains("~ update")
            || drifted.contains("+ create"),
        "{drifted}"
    );
    assert!(!drifted.to_lowercase().contains("password"), "{drifted}");
    let healed = engine::apply(root, &spec).expect("heal drift");
    assert!(healed.contains("Applied."), "{healed}");
    assert!(!healed.to_lowercase().contains("password"), "{healed}");
    assert!(
        docker::container_running(&cache),
        "apply should restart {cache}"
    );
    let clean = engine::plan_text(root, &spec).expect("plan after heal");
    assert!(clean.contains("No changes."), "{clean}");

    let destroyed = engine::destroy(root).expect("destroy");
    assert!(destroyed.contains("Destroyed"), "{destroyed}");
    assert!(!Path::new(&root.join(".tofy").join("outputs.env")).exists());
    assert!(!root.join(".tofy").join("main.tf.json").exists());
    assert!(!root.join(".tofy").join("docker-compose.yml").exists());
}

fn tofu_demo_spec() -> Project {
    let mut p = demo_spec();
    p.project = "tofy-tofu-smoke".into();
    p.backend = Backend::Tofu;
    p.resources[0].port = Some(55434);
    p.resources[1].port = Some(56380);
    p.resources[2].port = Some(59001);
    p
}

#[test]
fn smoke_apply_tofu_if_available() {
    if !docker::available() || !tofu::available() {
        eprintln!("skipping tofu smoke: Docker and OpenTofu engine are required");
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let spec = tofu_demo_spec();

    let planned = engine::plan_text(root, &spec).expect("tofu plan");
    assert!(
        looks_like_tofu_engine_plan(&planned),
        "tofu-backend plan must run tofu plan, not the house format: {planned}"
    );
    assert!(!looks_like_house_plan_only(&planned), "{planned}");
    assert!(!planned.contains("Applied."), "{planned}");
    assert!(!planned.to_lowercase().contains("go run tofu"), "{planned}");
    assert_plan_redacts_tf_secrets(root, &planned);
    let after_plan = tofy::state::State::load(root).unwrap();
    assert!(
        after_plan
            .resources
            .values()
            .all(|r| r.status != tofy::state::Status::Applied),
        "plan must not mark resources Applied"
    );

    let first = engine::apply(root, &spec).expect("tofu apply");
    assert!(first.contains("Applied."), "{first}");
    assert!(!first.to_lowercase().contains("go run tofu"));

    let outs = outputs::load(root).unwrap();
    assert!(outs["TOFY_APPDB_URI"].contains("@127.0.0.1:"));
    assert_eq!(
        outs.get("TOFY_UPLOADS_BUCKET").map(String::as_str),
        Some("uploads")
    );
    assert!(root.join(".tofy").join("main.tf.json").exists());
    assert!(root.join(".tofy").join("terraform.tfstate").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(root.join(".tofy").join("main.tf.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    let state = tofy::state::State::load(root).unwrap();
    assert_eq!(state.backend, Backend::Tofu);
    assert!(state
        .resources
        .values()
        .all(|r| r.status == tofy::state::Status::Applied));

    let cache = tofy::spec::replica_container(&spec.project, "cache", 0);
    assert!(
        std::process::Command::new("docker")
            .args(["stop", &cache])
            .status()
            .unwrap()
            .success(),
        "docker stop {cache}"
    );
    let drifted = engine::plan_text(root, &spec).expect("tofu plan after stop");
    assert!(
        looks_like_tofu_engine_plan(&drifted),
        "tofu-backend drift plan must be tofu plan: {drifted}"
    );
    assert!(!looks_like_house_plan_only(&drifted), "{drifted}");
    assert!(
        !is_tofu_no_changes(&drifted),
        "tofu plan ignored a stopped container: {drifted}"
    );
    assert!(!drifted.contains("Applied."), "{drifted}");
    assert_plan_redacts_tf_secrets(root, &drifted);
    assert!(
        !drifted.contains(&outs["TOFY_CACHE_PASSWORD"]),
        "plan leaked cache password"
    );
    let healed = engine::apply(root, &spec).expect("tofu heal drift");
    assert!(healed.contains("Applied."), "{healed}");
    assert!(!healed.to_lowercase().contains("go run tofu"), "{healed}");
    assert!(
        docker::container_running(&cache),
        "tofu apply should restart {cache}"
    );

    let destroyed = engine::destroy(root).expect("tofu destroy");
    assert!(destroyed.contains("Destroyed"), "{destroyed}");
    assert!(!root.join(".tofy").join("outputs.env").exists());
}

fn looks_like_tofu_engine_plan(text: &str) -> bool {
    text.contains("OpenTofu will perform")
        || text.contains("OpenTofu used the selected providers")
        || text.contains("Terraform will perform")
        || text.contains("docker_container")
        || text.contains(" to add,")
        || text.contains(" to change,")
        || text.contains(" to destroy")
        || text.contains("No changes. Your infrastructure")
}

fn assert_plan_redacts_tf_secrets(root: &Path, plan: &str) {
    let tf = std::fs::read_to_string(root.join(".tofy").join("main.tf.json"))
        .expect("main.tf.json after tofu plan");
    for prefix in [
        "POSTGRES_PASSWORD=",
        "MINIO_ROOT_PASSWORD=",
        "MINIO_ROOT_USER=",
    ] {
        for (i, _) in tf.match_indices(prefix) {
            let value: String = tf[i + prefix.len()..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect();
            if value.len() >= 4 {
                assert!(
                    !plan.contains(&value),
                    "plan leaked a secret from main.tf.json"
                );
            }
        }
    }
    if let Some(i) = tf.find("--requirepass") {
        let value: String = tf[i + "--requirepass".len()..]
            .chars()
            .skip_while(|c| !c.is_ascii_alphanumeric())
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        if value.len() >= 4 {
            assert!(
                !plan.contains(&value),
                "plan leaked the redis requirepass value"
            );
        }
    }
}

fn looks_like_house_plan_only(text: &str) -> bool {
    let house = text.contains("+ create  ") || text.trim() == "No changes.";
    house && !looks_like_tofu_engine_plan(text)
}

fn is_tofu_no_changes(text: &str) -> bool {
    text.contains("No changes. Your infrastructure") || text.trim() == "No changes."
}
