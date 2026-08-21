//! Builders that declare a [`tofy_spec::Project`].
//!
//! `postgres()` returns a declaration, not a live connection.

pub use crate::builder::{
    bucket, postgres, redis, stack, Applied, Bucket, Empty, NonEmpty, Open, Postgres, Redis, Stack,
};
pub use crate::main;
pub use tofy_spec::{Backend, Bind, Size};
