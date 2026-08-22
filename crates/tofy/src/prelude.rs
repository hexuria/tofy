//! Builders that declare a [`tofy_spec::Project`].
//!
//! `postgres()` returns a declaration, not a live connection.

pub use crate::builder::{
    bucket, mysql, postgres, redis, secret, stack, Applied, Bucket, Empty, Mysql, NonEmpty, Open,
    Postgres, Redis, Secret, Stack,
};
pub use crate::main;
pub use tofy_spec::{Backend, Bind, Size};
