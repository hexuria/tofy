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

    let destroyed = engine::destroy(root).expect("tofu destroy");
    assert!(destroyed.contains("Destroyed"), "{destroyed}");
    assert!(!root.join(".tofy").join("outputs.env").exists());
}
