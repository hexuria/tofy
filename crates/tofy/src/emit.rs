use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{json, Value};
use tofy_spec::{docker_network, replica_volume, Backend, Bind, Kind, Project};

use crate::aws;
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
            "memory_swap": r.size.docker_memory_swap_mb(),
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
    if !spec.backend.uses_opentofu() {
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

/// OpenTofu config for the selected engine. Contains secrets; mode `0600`, under `.tofy/`.
/// `Backend::Tofu` is the docker provider. `Backend::Aws` is the AWS provider.
pub fn write_tofu_config(root: &Path, spec: &Project, state: &mut State) -> Result<()> {
    write_tofu_config_mode(root, spec, state, aws::CidrMode::Rediscover)
}

pub(crate) fn write_tofu_config_mode(
    root: &Path,
    spec: &Project,
    state: &mut State,
    mode: aws::CidrMode,
) -> Result<()> {
    if spec.backend == Backend::Aws {
        aws::prepare_emit(spec, state, mode)?;
    }
    let dir = root.join(".tofy");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("main.tf.json");
    let tmp = dir.join("main.tf.json.tmp");
    let body = match spec.backend {
        Backend::Aws => aws_terraform_json(spec, state)?,
        Backend::Tofu | Backend::Local => terraform_json(spec, state),
    };
    std::fs::write(&tmp, serde_json::to_string_pretty(&body)?)?;
    set_private(&tmp)?;
    std::fs::rename(&tmp, &path)?;
    set_private(&path)?;
    Ok(())
}

/// AWS-provider OpenTofu JSON. Uses the account default VPC via data sources.
/// Does not create a VPC, subnet, load balancer, IAM user, or autoscaler.
/// Postgres / Redis get a tofy-owned security group. `Bind::Localhost` is the
/// applying machine's public IPv4 `/32`, not `127.0.0.1`. RDS is publicly
/// reachable from that `/32`. ElastiCache has no public IP, so Redis stays
/// VPC-only even with the same SG.
pub fn aws_terraform_json(spec: &Project, state: &State) -> Result<Value> {
    let mut required = serde_json::Map::new();
    required.insert(
        "aws".into(),
        json!({ "source": "hashicorp/aws", "version": "~> 5.0" }),
    );

    let mut provider = serde_json::Map::new();
    let mut aws_provider = serde_json::Map::new();
    if let Some(region) = aws::region() {
        aws_provider.insert("region".into(), json!(region));
    }
    provider.insert("aws".into(), Value::Object(aws_provider));

    let mut data = serde_json::Map::new();
    let mut resource = serde_json::Map::new();
    let mut outputs = serde_json::Map::new();

    let needs_vpc = aws::needs_engine_sg(spec);
    if needs_vpc {
        data.insert(
            "aws_vpc".into(),
            json!({
                "default": { "default": true }
            }),
        );
        data.insert(
            "aws_subnets".into(),
            json!({
                "default": {
                    "filter": [{
                        "name": "vpc-id",
                        "values": ["${data.aws_vpc.default.id}"]
                    }]
                }
            }),
        );
    }
    data.insert("aws_caller_identity".into(), json!({ "current": {} }));
    data.insert("aws_region".into(), json!({ "current": {} }));

    let applier_cidr = if aws::needs_applier_cidr(spec) {
        let cidr = state
            .applier_cidr
            .as_deref()
            .ok_or(crate::error::Error::PublicIpv4Undetermined)?;
        Some(cidr)
    } else {
        None
    };

    let mut db_instances = serde_json::Map::new();
    let mut db_subnet_groups = serde_json::Map::new();
    let mut cache_groups = serde_json::Map::new();
    let mut cache_subnet_groups = serde_json::Map::new();
    let mut buckets = serde_json::Map::new();
    let mut sg_ingress = serde_json::Map::new();

    for r in &spec.resources {
        let key = sanitize(&r.name);
        let rs = state.resources.get(&r.name);
        let outs = rs.map(|s| s.outputs.clone()).unwrap_or_default();
        let id = aws::resource_id(&spec.project, &r.name);
        match r.kind {
            Kind::Postgres => {
                let password = outs.get("password").cloned().unwrap_or_default();
                let user = outs.get("user").cloned().unwrap_or_else(|| "tofy".into());
                let database = outs
                    .get("database")
                    .cloned()
                    .unwrap_or_else(|| r.name.replace('-', "_"));
                let port = rs.map(|s| s.port).unwrap_or_else(|| r.port_or_default());
                sg_ingress.insert(
                    key.clone(),
                    json!({
                        "security_group_id": "${aws_security_group.tofy.id}",
                        "cidr_ipv4": ingress_cidr(r.bind, applier_cidr)?,
                        "from_port": port,
                        "to_port": port,
                        "ip_protocol": "tcp",
                        "description": format!("tofy postgres {}", r.name),
                    }),
                );
                db_subnet_groups.insert(
                    key.clone(),
                    json!({
                        "name": id,
                        "subnet_ids": "${data.aws_subnets.default.ids}",
                        "tags": { "tofy.project": spec.project, "tofy.resource": r.name },
                    }),
                );
                db_instances.insert(
                    key.clone(),
                    json!({
                        "identifier": id,
                        "engine": "postgres",
                        "engine_version": postgres_engine_version(r.version_or_default()),
                        "instance_class": r.size.aws_rds_instance_class(),
                        "allocated_storage": 20,
                        "storage_type": "gp3",
                        "username": user,
                        "password": password,
                        "db_name": database,
                        "port": port,
                        "db_subnet_group_name": format!("${{aws_db_subnet_group.{key}.name}}"),
                        "vpc_security_group_ids": ["${aws_security_group.tofy.id}"],
                        "publicly_accessible": true,
                        "multi_az": false,
                        "backup_retention_period": 0,
                        "skip_final_snapshot": true,
                        "deletion_protection": false,
                        "apply_immediately": true,
                        "tags": { "tofy.project": spec.project, "tofy.resource": r.name },
                    }),
                );
                outputs.insert(
                    format!("{}_host", r.name),
                    json!({ "value": format!("${{aws_db_instance.{key}.address}}") }),
                );
                outputs.insert(
                    format!("{}_port", r.name),
                    json!({ "value": format!("${{aws_db_instance.{key}.port}}") }),
                );
            }
            Kind::Redis => {
                let password = outs.get("password").cloned().unwrap_or_default();
                let port = rs.map(|s| s.port).unwrap_or_else(|| r.port_or_default());
                sg_ingress.insert(
                    key.clone(),
                    json!({
                        "security_group_id": "${aws_security_group.tofy.id}",
                        "cidr_ipv4": ingress_cidr(r.bind, applier_cidr)?,
                        "from_port": port,
                        "to_port": port,
                        "ip_protocol": "tcp",
                        "description": format!("tofy redis {}", r.name),
                    }),
                );
                cache_subnet_groups.insert(
                    key.clone(),
                    json!({
                        "name": id,
                        "subnet_ids": "${data.aws_subnets.default.ids}",
                        "tags": { "tofy.project": spec.project, "tofy.resource": r.name },
                    }),
                );
                cache_groups.insert(
                    key.clone(),
                    json!({
                        "replication_group_id": truncate_id(&id, 40),
                        "description": format!("tofy {} {}", spec.project, r.name),
                        "engine": "redis",
                        "engine_version": redis_engine_version(r.version_or_default()),
                        "node_type": r.size.aws_elasticache_node_type(),
                        "num_cache_clusters": 1,
                        "port": port,
                        "parameter_group_name": redis_parameter_group(r.version_or_default()),
                        "subnet_group_name": format!("${{aws_elasticache_subnet_group.{key}.name}}"),
                        "security_group_ids": ["${aws_security_group.tofy.id}"],
                        "automatic_failover_enabled": false,
                        "multi_az_enabled": false,
                        "transit_encryption_enabled": true,
                        "at_rest_encryption_enabled": true,
                        "auth_token": password,
                        "apply_immediately": true,
                        "tags": { "tofy.project": spec.project, "tofy.resource": r.name },
                    }),
                );
                outputs.insert(
                    format!("{}_host", r.name),
                    json!({ "value": format!("${{aws_elasticache_replication_group.{key}.primary_endpoint_address}}") }),
                );
                outputs.insert(
                    format!("{}_port", r.name),
                    json!({ "value": format!("${{aws_elasticache_replication_group.{key}.port}}") }),
                );
            }
            Kind::Bucket => {
                let bucket = outs
                    .get("bucket")
                    .cloned()
                    .unwrap_or_else(|| aws::s3_bucket_name(&spec.project, &r.name, "bucket"));
                buckets.insert(
                    key.clone(),
                    json!({
                        "bucket": bucket,
                        "force_destroy": true,
                        "tags": {
                            "tofy.project": spec.project,
                            "tofy.resource": r.name,
                            "tofy.size": r.size.as_str(),
                            "tofy.storage_class": r.size.aws_s3_storage_class(),
                        },
                    }),
                );
                outputs.insert(
                    format!("{}_bucket", r.name),
                    json!({ "value": format!("${{aws_s3_bucket.{key}.bucket}}") }),
                );
                outputs.insert(
                    format!("{}_region", r.name),
                    json!({ "value": "${data.aws_region.current.name}" }),
                );
                outputs.insert(
                    format!("{}_endpoint", r.name),
                    json!({ "value": format!("https://${{aws_s3_bucket.{key}.bucket}}.s3.${{data.aws_region.current.name}}.amazonaws.com") }),
                );
            }
        }
    }

    if needs_vpc {
        resource.insert(
            "aws_security_group".into(),
            json!({
                "tofy": {
                    "name": format!("tofy-{}", aws::aws_token(&spec.project)),
                    "description": "tofy applier access to postgres and redis",
                    "vpc_id": "${data.aws_vpc.default.id}",
                    "tags": {
                        "tofy.project": spec.project,
                        "tofy.role": "applier"
                    },
                }
            }),
        );
        resource.insert(
            "aws_vpc_security_group_ingress_rule".into(),
            Value::Object(sg_ingress),
        );
        resource.insert(
            "aws_vpc_security_group_egress_rule".into(),
            json!({
                "all": {
                    "security_group_id": "${aws_security_group.tofy.id}",
                    "cidr_ipv4": "0.0.0.0/0",
                    "ip_protocol": "-1",
                }
            }),
        );
    }
    if !db_instances.is_empty() {
        resource.insert(
            "aws_db_subnet_group".into(),
            Value::Object(db_subnet_groups),
        );
        resource.insert("aws_db_instance".into(), Value::Object(db_instances));
    }
    if !cache_groups.is_empty() {
        resource.insert(
            "aws_elasticache_subnet_group".into(),
            Value::Object(cache_subnet_groups),
        );
        resource.insert(
            "aws_elasticache_replication_group".into(),
            Value::Object(cache_groups),
        );
    }
    if !buckets.is_empty() {
        resource.insert("aws_s3_bucket".into(), Value::Object(buckets));
    }

    Ok(json!({
        "terraform": { "required_providers": required },
        "provider": provider,
        "data": data,
        "resource": resource,
        "output": outputs,
    }))
}

fn ingress_cidr<'a>(bind: Bind, applier: Option<&'a str>) -> Result<&'a str> {
    match bind {
        Bind::Localhost => applier.ok_or(crate::error::Error::PublicIpv4Undetermined),
        Bind::All => Ok("0.0.0.0/0"),
    }
}

fn postgres_engine_version(version: &str) -> String {
    version.split('.').next().unwrap_or("16").to_string()
}

fn redis_engine_version(version: &str) -> &'static str {
    match version.split('.').next().unwrap_or("7") {
        "6" => "6.2",
        _ => "7.1",
    }
}

fn redis_parameter_group(version: &str) -> &'static str {
    match version.split('.').next().unwrap_or("7") {
        "6" => "default.redis6",
        _ => "default.redis7",
    }
}

fn truncate_id(id: &str, max: usize) -> String {
    if id.len() <= max {
        id.to_string()
    } else {
        id[..max].trim_end_matches('-').to_string()
    }
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
        assert_eq!(
            tf["resource"]["docker_container"]["cache"]["memory_swap"],
            512
        );
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
        assert_eq!(c["memory_swap"], 1024);
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
        write_tofu_config(dir.path(), &spec, &mut state.clone()).unwrap();
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

    fn aws_spec() -> (Project, State) {
        let mut spec = Project::new("demoaws");
        spec.backend = Backend::Aws;
        spec.resources.push(
            Resource::new("appdb", Kind::Postgres)
                .with_port(25432)
                .with_size(tofy_spec::Size::Medium),
        );
        spec.resources
            .push(Resource::new("cache", Kind::Redis).with_port(26379));
        spec.resources.push(Resource::new("uploads", Kind::Bucket));
        let mut state = prepare_state(&spec, &State::default());
        state.applier_cidr = Some("203.0.113.10/32".into());
        (spec, state)
    }

    const APPLIER: &str = "203.0.113.10/32";

    #[test]
    fn aws_terraform_json_uses_aws_provider_not_docker() {
        let (spec, state) = aws_spec();
        let tf = aws_terraform_json(&spec, &state).unwrap();
        assert_eq!(
            tf["terraform"]["required_providers"]["aws"]["source"],
            "hashicorp/aws"
        );
        assert!(tf["terraform"]["required_providers"]
            .get("docker")
            .is_none());
        assert!(tf["provider"].get("docker").is_none());
        assert!(tf["resource"].get("docker_container").is_none());
        assert!(tf["resource"].get("aws_vpc").is_none());
        assert!(tf["resource"].get("aws_subnet").is_none());
        assert!(tf["resource"].get("aws_lb").is_none());
        assert!(tf["resource"].get("aws_iam_user").is_none());
        assert!(tf["resource"].get("aws_iam_role").is_none());
        assert!(tf["resource"].get("aws_autoscaling_group").is_none());
        assert_eq!(tf["data"]["aws_vpc"]["default"]["default"], true);
        let db = &tf["resource"]["aws_db_instance"]["appdb"];
        assert_eq!(db["instance_class"], "db.t4g.small");
        assert_eq!(db["engine"], "postgres");
        assert_eq!(db["multi_az"], false);
        assert_eq!(db["publicly_accessible"], true);
        assert_eq!(
            db["vpc_security_group_ids"][0],
            "${aws_security_group.tofy.id}"
        );
        let sg = &tf["resource"]["aws_security_group"]["tofy"];
        assert_eq!(sg["vpc_id"], "${data.aws_vpc.default.id}");
        assert!(sg["name"].as_str().unwrap().starts_with("tofy-"));
        let ingress = &tf["resource"]["aws_vpc_security_group_ingress_rule"];
        assert_eq!(ingress["appdb"]["cidr_ipv4"], APPLIER);
        assert_eq!(ingress["cache"]["cidr_ipv4"], APPLIER);
        assert_eq!(ingress["appdb"]["from_port"], 25432);
        assert_eq!(ingress["cache"]["from_port"], 26379);
        assert!(!ingress["appdb"]["cidr_ipv4"]
            .as_str()
            .unwrap()
            .contains("0.0.0.0/0"));
        assert_eq!(
            tf["resource"]["aws_elasticache_replication_group"]["cache"]["security_group_ids"][0],
            "${aws_security_group.tofy.id}"
        );
        assert_eq!(
            tf["resource"]["aws_elasticache_replication_group"]["cache"]["node_type"],
            "cache.t4g.micro"
        );
        assert_eq!(
            tf["resource"]["aws_elasticache_replication_group"]["cache"]["num_cache_clusters"],
            1
        );
        assert_eq!(
            tf["resource"]["aws_elasticache_replication_group"]["cache"]["multi_az_enabled"],
            false
        );
        assert_eq!(
            tf["resource"]["aws_elasticache_replication_group"]["cache"]
                ["transit_encryption_enabled"],
            true
        );
        let password = state.resources["cache"].outputs["password"].as_str();
        assert_eq!(
            tf["resource"]["aws_elasticache_replication_group"]["cache"]["auth_token"],
            password
        );
        let bucket = tf["resource"]["aws_s3_bucket"]["uploads"]["bucket"]
            .as_str()
            .unwrap();
        assert!(bucket.starts_with("tofy-demoaws-uploads-"), "{bucket}");
        assert_eq!(
            tf["resource"]["aws_s3_bucket"]["uploads"]["tags"]["tofy.storage_class"],
            "STANDARD"
        );
        assert!(tf["output"].get("appdb_host").is_some());
        assert!(tf["output"].get("uploads_endpoint").is_some());
    }

    #[test]
    fn aws_config_is_mode_0600_and_local_cleanup_keeps_it() {
        let dir = tempfile::tempdir().unwrap();
        let (spec, mut state) = aws_spec();
        crate::aws::with_applier_cidr(APPLIER, || {
            write_tofu_config(dir.path(), &spec, &mut state).unwrap();
        });
        let path = dir.path().join(".tofy").join("main.tf.json");
        assert!(path.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("hashicorp/aws"));
        assert!(!text.contains("kreuzwerker/docker"));
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(parsed["resource"].get("aws_vpc").is_none());
        assert!(parsed["data"]["aws_vpc"]["default"]["default"] == true);
        write_artifacts(dir.path(), &spec, &state).unwrap();
        assert!(
            path.exists(),
            "emit leftover cleanup must keep AWS tofu config"
        );
    }

    #[test]
    fn aws_size_large_maps_instance_class() {
        let mut spec = Project::new("demoaws");
        spec.backend = Backend::Aws;
        spec.resources
            .push(Resource::new("appdb", Kind::Postgres).with_size(tofy_spec::Size::Large));
        spec.resources
            .push(Resource::new("cache", Kind::Redis).with_size(tofy_spec::Size::Large));
        let mut state = prepare_state(&spec, &State::default());
        state.applier_cidr = Some(APPLIER.into());
        let tf = aws_terraform_json(&spec, &state).unwrap();
        assert_eq!(
            tf["resource"]["aws_db_instance"]["appdb"]["instance_class"],
            "db.t4g.medium"
        );
        assert_eq!(
            tf["resource"]["aws_elasticache_replication_group"]["cache"]["node_type"],
            "cache.t4g.medium"
        );
    }

    #[test]
    fn aws_localhost_sg_is_applier_slash32_not_world() {
        let (spec, state) = aws_spec();
        let tf = aws_terraform_json(&spec, &state).unwrap();
        let ingress = tf["resource"]["aws_vpc_security_group_ingress_rule"]
            .as_object()
            .unwrap();
        for (name, rule) in ingress {
            let cidr = rule["cidr_ipv4"].as_str().unwrap();
            assert!(cidr.ends_with("/32"), "{name} cidr={cidr}");
            assert_ne!(cidr, "0.0.0.0/0", "{name}");
        }
        assert!(tf["resource"].get("aws_vpc").is_none());
        assert_eq!(tf["data"]["aws_vpc"]["default"]["default"], true);
        assert!(tf["data"].get("aws_security_group").is_none());
        let bucket = tf["resource"]["aws_s3_bucket"]["uploads"]["bucket"]
            .as_str()
            .unwrap();
        assert!(bucket.starts_with("tofy-demoaws-uploads-"), "{bucket}");
        assert!(tf["resource"].get("aws_iam_user").is_none());
    }

    #[test]
    fn aws_bind_all_ingress_is_everywhere() {
        let mut spec = Project::new("demoaws");
        spec.backend = Backend::Aws;
        spec.resources
            .push(Resource::new("appdb", Kind::Postgres).with_bind(tofy_spec::Bind::All));
        spec.resources
            .push(Resource::new("cache", Kind::Redis).with_bind(tofy_spec::Bind::All));
        let state = prepare_state(&spec, &State::default());
        assert!(state.applier_cidr.is_none());
        let tf = aws_terraform_json(&spec, &state).unwrap();
        assert_eq!(
            tf["resource"]["aws_vpc_security_group_ingress_rule"]["appdb"]["cidr_ipv4"],
            "0.0.0.0/0"
        );
        assert_eq!(
            tf["resource"]["aws_vpc_security_group_ingress_rule"]["cache"]["cidr_ipv4"],
            "0.0.0.0/0"
        );
        assert_eq!(
            tf["resource"]["aws_db_instance"]["appdb"]["publicly_accessible"],
            true
        );
    }

    #[test]
    fn aws_s3_only_has_no_sg_and_skips_public_ip() {
        let mut spec = Project::new("demoaws");
        spec.backend = Backend::Aws;
        spec.resources.push(Resource::new("uploads", Kind::Bucket));
        let state = prepare_state(&spec, &State::default());
        let tf = aws_terraform_json(&spec, &state).unwrap();
        assert!(tf["resource"].get("aws_security_group").is_none());
        assert!(tf["resource"]
            .get("aws_vpc_security_group_ingress_rule")
            .is_none());
        assert!(tf["resource"].get("aws_db_instance").is_none());
        assert!(tf["resource"].get("aws_vpc").is_none());
        assert!(tf["resource"].get("aws_s3_bucket").is_some());
        assert!(tf["data"].get("aws_vpc").is_none());
    }

    #[test]
    fn aws_missing_public_ip_errors_and_does_not_open_the_world() {
        let mut spec = Project::new("demoaws");
        spec.backend = Backend::Aws;
        spec.resources.push(Resource::new("appdb", Kind::Postgres));
        let state = prepare_state(&spec, &State::default());
        let err = aws_terraform_json(&spec, &state).unwrap_err();
        assert!(matches!(err, crate::error::Error::PublicIpv4Undetermined));
        crate::aws::with_public_ip_undetermined(|| {
            let mut state = prepare_state(&spec, &State::default());
            let dir = tempfile::tempdir().unwrap();
            let err = write_tofu_config(dir.path(), &spec, &mut state).unwrap_err();
            assert!(matches!(err, crate::error::Error::PublicIpv4Undetermined));
            assert!(!dir.path().join(".tofy").join("main.tf.json").exists());
        });
    }

    #[test]
    fn aws_applier_ip_change_updates_sg_ingress() {
        let (spec, mut state) = aws_spec();
        let first = aws_terraform_json(&spec, &state).unwrap();
        state.applier_cidr = Some("198.51.100.20/32".into());
        let second = aws_terraform_json(&spec, &state).unwrap();
        assert_eq!(
            first["resource"]["aws_vpc_security_group_ingress_rule"]["appdb"]["cidr_ipv4"],
            "203.0.113.10/32"
        );
        assert_eq!(
            second["resource"]["aws_vpc_security_group_ingress_rule"]["appdb"]["cidr_ipv4"],
            "198.51.100.20/32"
        );
        assert_eq!(
            first["resource"]["aws_s3_bucket"]["uploads"]["bucket"],
            second["resource"]["aws_s3_bucket"]["uploads"]["bucket"]
        );
    }
}
