These programs must not compile. `cargo test -p tofy typestate_compile_fail` runs them through trybuild.

`.stderr` fixtures match rustc 1.83 (see `rust-toolchain.toml`). CI installs that same toolchain so diagnostic wording does not drift.

- `postgres_replicas.rs` — `Postgres<Open>` has no `replicas`
- `mysql_replicas.rs` — `Mysql<Open>` has no `replicas`
- `redis_replicas.rs` — `Redis<Open>` has no `replicas`
- `bucket_replicas.rs` — `Bucket<Open>` has no `replicas`
- `empty_apply.rs` — `Stack<Empty>` has no `apply`
- `applied_add.rs` — `Stack<Applied>` has no `add`
