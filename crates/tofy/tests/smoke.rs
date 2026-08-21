//! Optional Docker smoke test. Skips when Docker is not available.

use std::path::Path;

use tofy::docker;
use tofy::engine;
use tofy::outputs;
use tofy::spec::{Kind, Project, Resource};

fn demo_spec() -> Project {
    let mut p = Project::new("tofy-smoke");
    p.resources.push(Resource {
        name: "appdb".into(),
        kind: Kind::Postgres,
        version: Some("16".into()),
        port: Some(55433),
    });
    p.resources.push(Resource {
        name: "cache".into(),
        kind: Kind::Redis,
        version: None,
        port: Some(56379),
    });
    p.resources.push(Resource {
        name: "uploads".into(),
        kind: Kind::Bucket,
        version: None,
        port: Some(59000),
    });
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
    assert!(outs.contains_key("TOFY_UPLOADS_SECRET_KEY"));
    let password = outs["TOFY_APPDB_PASSWORD"].clone();
    assert_ne!(password, "tofy-tofy-smoke-appdb");
    assert!(!password.starts_with("tofy-"));

    assert!(root.join(".tofy").join("state.json").exists());
    assert!(root.join(".tofy").join("outputs.env").exists());
    assert!(root.join(".tofy").join("spec.json").exists());

    let second = engine::apply(root, &spec).expect("second apply");
    assert!(second.contains("No changes.") || !second.contains("+ create"), "{second}");
    let outs2 = outputs::load(root).unwrap();
    assert_eq!(password, outs2["TOFY_APPDB_PASSWORD"]);

    let destroyed = engine::destroy(root).expect("destroy");
    assert!(destroyed.contains("Destroyed"), "{destroyed}");
    assert!(!Path::new(&root.join(".tofy").join("outputs.env")).exists());
    assert!(root.join(".tofy").join("main.tf.json").exists());
}
