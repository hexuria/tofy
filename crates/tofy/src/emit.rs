use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{json, Value};
use tofy_spec::{docker_network, replica_volume, Backend, Kind, Project};

use crate::error::Result;
use crate::state::{docker_image, set_private, State};

pub fn terraform_json(spec: &Project, state: &State) -> Value {
    let mut required = serde_json::Map::new();
    required.insert(
        "docker".into(),
        json!({ "source": "kreuzwerker/docker", "version": "~> 3.6" }),
    );

    let mut containers = serde_json::Map::new();
    let mut images = serde_json::Map::new();
    let mut volumes = serde_json::Map::new();
    let net = docker_network(&spec.project);

    for r in &spec.resources {
        let image = docker_image(r);
        let key = sanitize(&r.name);
        images.insert(key.clone(), json!({ "name": image, "keep_locally": true }));
        let rs = state.resources.get(&r.name);
        let outs = rs.map(|s| s.outputs.clone()).unwrap_or_default();
        let host_port = rs.map(|s| s.port).unwrap_or_else(|| r.port_or_default());
        let (env, cmd) = container_env(r.kind, &outs);
        let mut c = json!({
            "name": format!("tofy-{}-{}", spec.project, r.name),
            "image": format!("${{docker_image.{key}.image_id}}"),
            "hostname": r.name,
            "restart": "unless-stopped",
            "must_run": true,
            "memory": r.size.docker_memory_mb(),
            "cpus": r.size.docker_cpus(),
            "ports": [{
                "internal": r.kind.internal_port(),
                "external": host_port,
                "ip": r.bind.as_ip(),
            }],
            "env": env,
            "networks_advanced": [{
                "name": "${docker_network.stack.name}",
                "aliases": [r.name.clone()],
            }],
            "labels": [
                { "label": "tofy.project", "value": spec.project },
                { "label": "tofy.resource", "value": r.name },
                { "label": "tofy.replica", "value": "1" },
            ],
        });
        if let Some(cmd) = cmd {
            c["command"] = json!(cmd);
        }
        if matches!(r.kind, Kind::Postgres | Kind::Bucket) {
            let vol = replica_volume(&spec.project, &r.name, 0);
            volumes.insert(key.clone(), json!({ "name": vol }));
            let mount = match r.kind {
                Kind::Postgres => "/var/lib/postgresql/data",
                Kind::Bucket => "/data",
                Kind::Redis => unreachable!(),
            };
            c["volumes"] = json!([{
                "volume_name": format!("${{docker_volume.{key}.name}}"),
                "container_path": mount,
            }]);
        }
        containers.insert(key, c);
    }

    let mut resource = serde_json::Map::new();
    if !spec.resources.is_empty() {
        resource.insert(
            "docker_network".into(),
            json!({
                "stack": {
                    "name": net,
                    "labels": [{ "label": "tofy.project", "value": spec.project }],
                }
            }),
        );
        resource.insert("docker_image".into(), Value::Object(images));
        if !volumes.is_empty() {
            resource.insert("docker_volume".into(), Value::Object(volumes));
        }
        resource.insert("docker_container".into(), Value::Object(containers));
    }

    json!({
        "terraform": { "required_providers": required },
        "provider": { "docker": { "host": docker_host() } },
        "resource": resource,
    })
}

fn docker_host() -> String {
    std::env::var("DOCKER_HOST").unwrap_or_else(|_| "unix:///var/run/docker.sock".into())
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

/// Write the language-agnostic spec JSON only.
///
/// Local apply does **not** write `docker-compose.yml` or `main.tf.json`.
/// Those would embed live passwords as a world-readable default artifact.
/// The OpenTofu backend writes `main.tf.json` separately (mode `0600`).
pub fn write_artifacts(root: &Path, spec: &Project, _state: &State) -> Result<()> {
    let dir = root.join(".tofy");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("spec.json"), spec.to_json_pretty()?)?;
    let mut leftover = vec!["docker-compose.yml"];
    if spec.backend != Backend::Tofu {
        leftover.push("main.tf.json");
    }
    for name in leftover {
        let p = dir.join(name);
        if p.exists() {
            std::fs::remove_file(p)?;
        }
    }
    Ok(())
}

/// OpenTofu docker-provider config. Contains secrets; mode `0600`, under `.tofy/`.
pub fn write_tofu_config(root: &Path, spec: &Project, state: &State) -> Result<()> {
    let dir = root.join(".tofy");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("main.tf.json");
    let tmp = dir.join("main.tf.json.tmp");
    std::fs::write(
        &tmp,
        serde_json::to_string_pretty(&terraform_json(spec, state))?,
    )?;
    set_private(&tmp)?;
    std::fs::rename(&tmp, &path)?;
    set_private(&path)?;
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
        Kind::Redis => {
            let password = outs.get("password").map(String::as_str).unwrap_or("");
            (
                vec![],
                Some(vec![
                    "redis-server".into(),
                    "--requirepass".into(),
                    password.into(),
                ]),
            )
        }
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
        let cache_cmd = tf["resource"]["docker_container"]["cache"]["command"]
            .as_array()
            .expect("redis command");
        assert_eq!(cache_cmd[0], "redis-server");
        assert_eq!(cache_cmd[1], "--requirepass");
        assert!(!cache_cmd[2].as_str().unwrap_or("").is_empty());
        assert!(tf["resource"]["docker_volume"]
            .as_object()
            .unwrap()
            .contains_key("uploads"));
        assert!(!tf["resource"]["docker_volume"]
            .as_object()
            .unwrap()
            .contains_key("cache"));
        assert_eq!(tf["resource"]["docker_container"]["cache"]["memory"], 256);
        assert_eq!(tf["resource"]["docker_container"]["cache"]["cpus"], "0.25");
    }

    #[test]
    fn terraform_json_postgres_volume_and_bind() {
        let mut spec = Project::new("demo");
        spec.resources.push(
            Resource::new("appdb", Kind::Postgres)
                .with_port(5433)
                .with_size(tofy_spec::Size::Medium),
        );
        let state = prepare_state(&spec, &State::default());
        let tf = terraform_json(&spec, &state);
        let c = &tf["resource"]["docker_container"]["appdb"];
        assert_eq!(c["ports"][0]["external"], 5433);
        assert_eq!(c["ports"][0]["ip"], "127.0.0.1");
        assert_eq!(c["memory"], 512);
        assert_eq!(c["cpus"], "0.50");
        assert_eq!(
            tf["resource"]["docker_volume"]["appdb"]["name"],
            "tofy-demo-appdb-data"
        );
        assert_eq!(
            c["volumes"][0]["container_path"],
            "/var/lib/postgresql/data"
        );
        assert_eq!(c["hostname"], "appdb");
        assert!(c["networks_advanced"][0]["aliases"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a == "appdb"));
    }

    #[test]
    fn apply_artifacts_are_spec_only() {
        let dir = tempfile::tempdir().unwrap();
        let mut spec = Project::new("demo");
        spec.resources.push(Resource::new("appdb", Kind::Postgres));
        spec.resources.push(Resource::new("cache", Kind::Redis));
        spec.resources.push(Resource::new("uploads", Kind::Bucket));
        let state = prepare_state(&spec, &State::default());
        write_artifacts(dir.path(), &spec, &state).unwrap();
        assert!(dir.path().join(".tofy").join("spec.json").exists());
        assert!(!dir.path().join(".tofy").join("main.tf.json").exists());
        assert!(!dir.path().join(".tofy").join("docker-compose.yml").exists());
        let spec_text =
            std::fs::read_to_string(dir.path().join(".tofy").join("spec.json")).unwrap();
        assert!(!spec_text.to_lowercase().contains("password"));
        assert!(!spec_text.contains("POSTGRES_PASSWORD"));
    }

    #[test]
    fn tofu_config_is_mode_0600_and_gitignored_path() {
        let dir = tempfile::tempdir().unwrap();
        let mut spec = Project::new("demo");
        spec.backend = tofy_spec::Backend::Tofu;
        spec.resources.push(Resource::new("cache", Kind::Redis));
        spec.resources.push(Resource::new("uploads", Kind::Bucket));
        let state = prepare_state(&spec, &State::default());
        write_tofu_config(dir.path(), &spec, &state).unwrap();
        let path = dir.path().join(".tofy").join("main.tf.json");
        assert!(path.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("requirepass"));
        assert!(text.contains("/data"));
        assert!(text.contains("kreuzwerker/docker"));
        write_artifacts(dir.path(), &spec, &state).unwrap();
        assert!(
            path.exists(),
            "local leftover cleanup must not drop tofu config"
        );
    }
}
