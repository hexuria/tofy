#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Spec(#[from] tofy_spec::SpecError),
    #[error("{0}")]
    Engine(String),
    #[error("Docker is not available; emitted artifacts to .tofy but did not apply.")]
    DockerMissing,
    #[error("Docker is not available; did not destroy. State left unchanged.")]
    DestroyNeedsDocker,
    #[error("OpenTofu engine is required for this backend; did not apply.")]
    TofuMissing,
    #[error("OpenTofu engine is required for this backend; did not plan.")]
    PlanNeedsTofu,
    #[error(
        "OpenTofu engine is required for this backend; did not destroy. State left unchanged."
    )]
    DestroyNeedsTofu,
    #[error(
        "another tofy apply or destroy is already running in this directory; did not apply or destroy"
    )]
    Locked,
    #[error("{0}")]
    Usage(String),
}

impl Error {
    pub fn exit_code(&self) -> i32 {
        1
    }
}

pub type Result<T> = std::result::Result<T, Error>;
