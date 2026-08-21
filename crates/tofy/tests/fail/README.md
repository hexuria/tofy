These programs must not compile. `cargo test -p tofy typestate_compile_fail` runs them through trybuild.

- `postgres_replicas.rs` — `Postgres<Open>` has no `replicas`
- `empty_apply.rs` — `Stack<Empty>` has no `apply`
- `applied_add.rs` — `Stack<Applied>` has no `add`
