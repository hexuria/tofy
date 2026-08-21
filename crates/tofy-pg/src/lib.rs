//! Optional live [`sqlx::PgPool`] after a tofy apply.
//!
//! Other languages keep reading `TOFY_*`. This crate is Rust-only and is not
//! the default consume path. `#[tofy::main]` stays sync.

use std::collections::BTreeMap;
use std::path::Path;

use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use tofy_spec::env_var;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("missing env {0}")]
    MissingEnv(String),
    #[error("no outputs. run `tofy apply` first")]
    NoOutputs,
    #[error("no postgres URI for resource {0}")]
    MissingUri(String),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Read `TOFY_<NAME>_URI` (or any env var) without connecting.
pub fn uri_from_env(var: &str) -> Result<String> {
    std::env::var(var).map_err(|_| Error::MissingEnv(var.to_string()))
}

/// Read the resource host URI from `.tofy/outputs.json` without connecting.
pub fn uri_from_outputs(root: &Path, resource: &str) -> Result<String> {
    let path = root.join(".tofy").join("outputs.json");
    if !path.exists() {
        return Err(Error::NoOutputs);
    }
    let map: BTreeMap<String, String> = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    let key = env_var(resource, "uri");
    map.get(&key)
        .cloned()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Error::MissingUri(resource.to_string()))
}

pub async fn pool_from_uri(uri: &str) -> Result<PgPool> {
    Ok(PgPoolOptions::new().connect(uri).await?)
}

pub async fn pool_from_env(var: &str) -> Result<PgPool> {
    pool_from_uri(&uri_from_env(var)?).await
}

pub async fn pool_from_outputs(root: &Path, resource: &str) -> Result<PgPool> {
    pool_from_uri(&uri_from_outputs(root, resource)?).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_from_outputs_reads_flattened_key() {
        let dir = tempfile::tempdir().unwrap();
        let tofy = dir.path().join(".tofy");
        std::fs::create_dir_all(&tofy).unwrap();
        std::fs::write(
            tofy.join("outputs.json"),
            r#"{"TOFY_APPDB_URI":"postgres://tofy:s3cret@127.0.0.1:5433/appdb","TOFY_APPDB_PASSWORD":"s3cret"}"#,
        )
        .unwrap();
        let uri = uri_from_outputs(dir.path(), "appdb").unwrap();
        assert_eq!(uri, "postgres://tofy:s3cret@127.0.0.1:5433/appdb");
    }

    #[test]
    fn uri_from_outputs_missing_file_is_no_outputs() {
        let dir = tempfile::tempdir().unwrap();
        let err = uri_from_outputs(dir.path(), "appdb").unwrap_err();
        assert!(matches!(err, Error::NoOutputs));
    }

    #[test]
    fn uri_from_outputs_missing_resource() {
        let dir = tempfile::tempdir().unwrap();
        let tofy = dir.path().join(".tofy");
        std::fs::create_dir_all(&tofy).unwrap();
        std::fs::write(
            tofy.join("outputs.json"),
            r#"{"TOFY_CACHE_URI":"redis://:x@127.0.0.1:6379"}"#,
        )
        .unwrap();
        let err = uri_from_outputs(dir.path(), "appdb").unwrap_err();
        assert!(matches!(err, Error::MissingUri(_)));
    }

    #[test]
    fn uri_from_env_reads_var() {
        std::env::set_var("TOFY_TEST_URI", "postgres://tofy:x@127.0.0.1:5433/appdb");
        let uri = uri_from_env("TOFY_TEST_URI").unwrap();
        assert!(uri.starts_with("postgres://"));
        std::env::remove_var("TOFY_TEST_URI");
    }
}
