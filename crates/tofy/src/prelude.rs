//! Builders that declare a [`tofy_spec::Project`].
//!
//! `postgres()` returns a declaration, not a live connection.

pub use crate::builder::{bucket, postgres, redis, stack, Bucket, Postgres, Redis, Stack};
pub use tofy_spec::{Bind, Size};
