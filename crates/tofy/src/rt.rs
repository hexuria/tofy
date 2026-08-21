use crate::builder;
use crate::error::{Error, Result};

/// Called by `#[tofy::main]` after the user `main` returns.
///
/// Apply happens in `stack(...).apply()`, not here. This only catches a
/// declared stack that was never sealed.
pub fn finish() -> Result<()> {
    if builder::stack_left_open() {
        return Err(Error::Usage(
            "stack() was declared but apply() was not called".into(),
        ));
    }
    Ok(())
}
