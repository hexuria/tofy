use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{json, Value};

use crate::spec::{Kind, Project};

pub fn terraform_json(spec: &Project) -> Value {
    let mut required = serde_json::Map::new();
    required.insert(
        "docker".into(),
        json!({ "source": "kreuzwerker/docker", "version": "~> 3.0" }),
    );

    let mut containers = serde_json::Map::new();
    let mut images = serde_json::Map::new();
    let mut outputs = serde_json::Map::new();

    for r in &spec.resources {
        let image = r.image();
        images.insert(
            sanitize(&r.name),
            json!({ "name": image, "keep_locally": true }),
        );
        let (env, cmd) = container_env(spec, r);
        let mut c = json!({
            "name": format!("tofy-{}-{}", spec.project, r.name),
            "image": image,
            "ports": [{ "internal": internal_port(r.kind.clone()), "external": r.default_port() }],
            "env": env,
            "must_run": true,
        });
        if let Some(cmd) = cmd {
            c["command"] = json!(cmd);
        }
        containers.insert(sanitize(&r.name), c);

        for (k, v) in crate::state::outputs_for(spec, r) {
            outputs.insert(
                format!("{}_{}", sanitize(&r.name), k),
                json!({ "value": v, "sensitive": k.contains("password") || k.contains("secret") || k == "uri" }),
            );
        }
    }

    json!({
        "terraform": { "required_providers": required },
        "provider": { "docker": { "host": "unix:///var/run/docker.sock" } },
        "resource": {
            "docker_image": images,
            "docker_container": containers,
        },
        "output": outputs,
    })
}

pub fn compose_yaml(spec: &Project) -> String {
    let mut s = String::from("services:\n");
    for r in &spec.resources {
        let (env, cmd) = container_env(spec, r);
        s.push_str(&format!("  {}:\n", r.name));
        s.push_str(&format!("    image: {}\n", r.image()));
        s.push_str(&format!(
            "    ports:\n      - \"{}:{}\"\n",
            r.default_port(),
            internal_port(r.kind.clone())
        ));
        s.push_str("    environment:\n");
        for e in env {
            let (k, v) = e.split_once('=').unwrap_or((e.as_str(), ""));
            s.push_str(&format!("      {k}: \"{v}\"\n"));
        }
        if let Some(cmd) = cmd {
            s.push_str(&format!("    command: {}\n", cmd.join(" ")));
        }
    }
    s
}

pub fn write_artifacts(root: &Path, spec: &Project) -> Result<(), crate::Error> {
    let dir = root.join(".tofy");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join("main.tf.json"),
        serde_json::to_string_pretty(&terraform_json(spec))?,
    )?;
    std::fs::write(root.join("docker-compose.yml"), compose_yaml(spec))?;
    let mut outs = BTreeMap::new();
    for r in &spec.resources {
        outs.insert(r.name.clone(), crate::state::outputs_for(spec, r));
    }
    std::fs::write(dir.join("outputs.json"), serde_json::to_string_pretty(&outs)?)?;
    Ok(())
}

fn sanitize(name: &str) -> String {
    name.replace('-', "_")
}

fn internal_port(kind: Kind) -> u16 {
    match kind {
        Kind::Postgres => 5432,
        Kind::Redis => 6379,
        Kind::Bucket => 9000,
    }
}

fn container_env(spec: &Project, r: &crate::spec::Resource) -> (Vec<String>, Option<Vec<String>>) {
    match r.kind {
        Kind::Postgres => {
            let outs = crate::state::outputs_for(spec, r);
            (
                vec![
                    format!("POSTGRES_USER={}", outs["user"]),
                    format!("POSTGRES_PASSWORD={}", outs["password"]),
                    format!("POSTGRES_DB={}", outs["database"]),
                ],
                None,
            )
        }
        Kind::Redis => (vec![], None),
        Kind::Bucket => {
            let outs = crate::state::outputs_for(spec, r);
            (
                vec![
                    format!("MINIO_ROOT_USER={}", outs["access_key"]),
                    format!("MINIO_ROOT_PASSWORD={}", outs["secret_key"]),
                ],
                Some(vec!["server".into(), "/data".into(), "--console-address".into(), ":9001".into()]),
            )
        }
    }
}
