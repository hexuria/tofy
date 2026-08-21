use crate::builder;
use crate::cli;
use crate::error::{Error, Result};

/// Called by `#[tofy::main]` after the stack is declared.
pub fn dispatch() -> Result<()> {
    let project = builder::take_project().ok_or_else(|| {
        Error::Spec(tofy_spec::SpecError::Validation(
            "stack() was not called".into(),
        ))
    })?;
    cli::run_with_project(project)
}
