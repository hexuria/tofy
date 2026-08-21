use std::collections::BTreeMap;
use std::path::Path;

use tofy_spec::{env_var, is_secret_key};

use crate::error::Result;
use crate::state::{set_private, State};

pub fn flatten(state: &State) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if !state.project.is_empty() && state.backend != tofy_spec::Backend::Aws {
        out.insert(
            "TOFY_NETWORK".into(),
            tofy_spec::docker_network(&state.project),
        );
    }
    for (name, res) in &state.resources {
        for (key, value) in &res.outputs {
            out.insert(env_var(name, key), value.clone());
        }
    }
    out
}

pub fn write(root: &Path, state: &State) -> Result<()> {
    let dir = root.join(".tofy");
    std::fs::create_dir_all(&dir)?;
    let flat = flatten(state);

    let json_path = dir.join("outputs.json");
    std::fs::write(&json_path, serde_json::to_string_pretty(&flat)?)?;
    set_private(&json_path)?;

    let mut env = String::new();
    for (k, v) in &flat {
        env.push_str(&format!("{k}={v}\n"));
    }
    let env_path = dir.join("outputs.env");
    std::fs::write(&env_path, env)?;
    set_private(&env_path)?;
    Ok(())
}

pub fn load(root: &Path) -> Result<BTreeMap<String, String>> {
    let path = root.join(".tofy").join("outputs.json");
    if !path.exists() {
        return Err(crate::error::Error::Engine(
            "no outputs. run `tofy apply` first".into(),
        ));
    }
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}

pub fn format_public(map: &BTreeMap<String, String>) -> String {
    let mut s = String::new();
    for (k, v) in map {
        if is_secret_key(k) {
            continue;
        }
        s.push_str(&format!("{k}={v}\n"));
    }
    s
}

pub fn redact_value(key: &str, value: &str) -> String {
    if is_secret_key(key) {
        "(redacted)".into()
    } else {
        value.to_string()
    }
}

pub fn clear(root: &Path) -> Result<()> {
    for name in ["outputs.json", "outputs.env"] {
        let p = root.join(".tofy").join(name);
        if p.exists() {
            std::fs::remove_file(p)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{prepare_state, State};
    use tofy_spec::{Kind, Project, Resource};

    #[test]
    fn flatten_uses_tofy_prefix() {
        let mut spec = Project::new("demo");
        spec.resources.push(
            Resource::new("appdb", Kind::Postgres)
                .with_version("16")
                .with_port(5433),
        );
        spec.resources.push(Resource::new("cache", Kind::Redis));
        let state = prepare_state(&spec, &State::default());
        let flat = flatten(&state);
        assert!(flat.contains_key("TOFY_APPDB_URI"));
        assert!(flat.contains_key("TOFY_APPDB_PASSWORD"));
        assert!(flat.contains_key("TOFY_CACHE_URI"));
        assert!(flat.contains_key("TOFY_CACHE_PASSWORD"));
        assert_eq!(flat["TOFY_NETWORK"], "tofy-demo");
        assert!(flat["TOFY_APPDB_URI"].contains("@127.0.0.1:"));
        assert!(flat["TOFY_APPDB_INTERNAL_URI"].contains("@appdb:5432/"));
        assert!(flat["TOFY_CACHE_URI"].starts_with("redis://:"));
        assert!(flat["TOFY_CACHE_URI"].contains(&flat["TOFY_CACHE_PASSWORD"]));
        let public = format_public(&flat);
        assert!(!public.contains("PASSWORD"));
        assert!(!public.contains(&flat["TOFY_APPDB_PASSWORD"]));
        assert!(!public.contains(&flat["TOFY_CACHE_PASSWORD"]));
        assert!(public.contains("TOFY_APPDB_PORT=5433"));
        assert!(public.contains("TOFY_CACHE_PORT=6379"));
        assert!(public.contains("TOFY_APPDB_INTERNAL_HOST=appdb"));
        assert!(public.contains("TOFY_NETWORK=tofy-demo"));
    }
}
