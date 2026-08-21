use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{json, Value};
use tofy_spec::{docker_network, Kind, Project};

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
    let net = docker_network(&spec.project);

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
                "ip": r.bind.as_ip(),
            }],
            "env": env,
            "must_run": true,
            "memory": r.size.docker_memory(),
            "cpu_shares": match r.size.as_str() {
                "small" => 256,
                "medium" => 512,
                _ => 1024,
            },
            "networks_advanced": [{
                "name": net,
                "aliases": [r.name.clone()],
            }],
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
            "docker_network": {
                "stack": {
                    "name": net,
                    "labels": [{ "label": "tofy.project", "value": spec.project }],
                }
            },
            "docker_image": images,
            "docker_container": containers,
        },
        "output": outputs,
    })
}

pub fn compose_yaml(spec: &Project, state: &State) -> String {
    let net = docker_network(&spec.project);
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
            "    container_name: {}\n",
            tofy_spec::container_name(&spec.project, &r.name)
        ));
        s.push_str(&format!("    hostname: {}\n", r.name));
        s.push_str(&format!("    mem_limit: {}\n", r.size.docker_memory()));
        s.push_str(&format!("    cpus: {}\n", r.size.docker_cpus()));
        s.push_str(&format!(
            "    ports:\n      - \"{}:{}:{}\"\n",
            r.bind.as_ip(),
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
        s.push_str("    networks:\n");
        s.push_str(&format!(
            "      stack:\n        aliases:\n          - {}\n",
            r.name
        ));
    }
    s.push_str("networks:\n");
    s.push_str("  stack:\n");
    s.push_str(&format!("    name: {net}\n"));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::prepare_state;
    use tofy_spec::{Kind, Resource};

    #[test]
    fn compose_and_terraform_emit_one_container_per_resource() {
        let mut spec = Project::new("demo");
        spec.resources
            .push(Resource::new("cache", Kind::Redis).with_replicas(1));
        spec.resources
            .push(Resource::new("uploads", Kind::Bucket).with_replicas(1));
        let state = prepare_state(&spec, &State::default());
        let compose = compose_yaml(&spec, &state);
        let tf = terraform_json(&spec, &state);
        assert_eq!(compose.matches("container_name:").count(), 2, "{compose}");
        assert!(!compose.contains("cache-2"), "{compose}");
        assert!(!compose.contains("uploads-2"), "{compose}");
        let containers = tf["resource"]["docker_container"].as_object().unwrap();
        assert_eq!(containers.len(), 2);
        assert!(containers.contains_key("cache"));
        assert!(containers.contains_key("uploads"));
    }
}
