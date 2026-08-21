use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{json, Value};
use tofy_spec::{Kind, Project};

use crate::error::Result;
use crate::state::{docker_image, State};

pub fn terraform_json(spec: &Project, state: &State) -> Value {
    let mut required = serde_json::Map::new();
    required.insert(
        "docker".into(),
        json!({ "source": "kreuzwerker/docker", "version": "~> 3.0" }),
    );

    let mut containers = serde_json::Map::new();
    let mut images = serde_json::Map::new();
    let mut outputs = serde_json::Map::new();

    for r in &spec.resources {
        let image = docker_image(r);
        let key = sanitize(&r.name);
        images.insert(key.clone(), json!({ "name": image, "keep_locally": true }));
        let outs = state
            .resources
            .get(&r.name)
            .map(|s| s.outputs.clone())
            .unwrap_or_default();
        let (env, cmd) = container_env(r.kind, &outs);
        let mut c = json!({
            "name": format!("tofy-{}-{}", spec.project, r.name),
            "image": image,
            "ports": [{
                "internal": r.kind.internal_port(),
                "external": r.port_or_default(),
                "ip": "127.0.0.1",
            }],
            "env": env,
            "must_run": true,
        });
        if let Some(cmd) = cmd {
            c["command"] = json!(cmd);
        }
        containers.insert(key.clone(), c);

        for (k, v) in &outs {
            outputs.insert(
                format!("{}_{}", key, sanitize(k)),
                json!({
                    "value": v,
                    "sensitive": tofy_spec::is_secret_key(k),
                }),
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

pub fn compose_yaml(spec: &Project, state: &State) -> String {
    let mut s = String::from("services:\n");
    for r in &spec.resources {
        let outs = state
            .resources
            .get(&r.name)
            .map(|st| st.outputs.clone())
            .unwrap_or_default();
        let (env, cmd) = container_env(r.kind, &outs);
        s.push_str(&format!("  {}:\n", r.name));
        s.push_str(&format!("    image: {}\n", docker_image(r)));
        s.push_str(&format!(
            "    container_name: tofy-{}-{}\n",
            spec.project, r.name
        ));
        s.push_str(&format!(
            "    ports:\n      - \"127.0.0.1:{}:{}\"\n",
            r.port_or_default(),
            r.kind.internal_port()
        ));
        if !env.is_empty() {
            s.push_str("    environment:\n");
            for e in env {
                let (k, v) = e.split_once('=').unwrap_or((e.as_str(), ""));
                s.push_str(&format!("      {k}: \"{v}\"\n"));
            }
        }
        if let Some(cmd) = cmd {
            s.push_str(&format!("    command: {}\n", cmd.join(" ")));
        }
    }
    s
}

pub fn write_artifacts(root: &Path, spec: &Project, state: &State) -> Result<()> {
    let dir = root.join(".tofy");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("spec.json"), spec.to_json_pretty()?)?;
    std::fs::write(
        dir.join("main.tf.json"),
        serde_json::to_string_pretty(&terraform_json(spec, state))?,
    )?;
    std::fs::write(dir.join("docker-compose.yml"), compose_yaml(spec, state))?;
    Ok(())
}

fn sanitize(name: &str) -> String {
    name.replace('-', "_")
}

fn container_env(
    kind: Kind,
    outs: &BTreeMap<String, String>,
) -> (Vec<String>, Option<Vec<String>>) {
    match kind {
        Kind::Postgres => (
            vec![
                format!(
                    "POSTGRES_USER={}",
                    outs.get("user").map(String::as_str).unwrap_or("tofy")
                ),
                format!(
                    "POSTGRES_PASSWORD={}",
                    outs.get("password").map(String::as_str).unwrap_or("")
                ),
                format!(
                    "POSTGRES_DB={}",
                    outs.get("database").map(String::as_str).unwrap_or("app")
                ),
            ],
            None,
        ),
        Kind::Redis => (vec![], None),
        Kind::Bucket => (
            vec![
                format!(
                    "MINIO_ROOT_USER={}",
                    outs.get("access_key").map(String::as_str).unwrap_or("")
                ),
                format!(
                    "MINIO_ROOT_PASSWORD={}",
                    outs.get("secret_key").map(String::as_str).unwrap_or("")
                ),
            ],
            Some(vec![
                "server".into(),
                "/data".into(),
                "--console-address".into(),
                ":9001".into(),
            ]),
        ),
    }
}
