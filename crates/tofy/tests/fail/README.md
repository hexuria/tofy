These programs must not compile. `cargo test -p tofy typestate_compile_fail` runs them through trybuild.

`.stderr` fixtures match rustc 1.83 (see `rust-toolchain.toml`). CI installs that same toolchain so diagnostic wording does not drift.

- `bucket_replicas.rs` — `Bucket<Open>` has no `replicas`
- `secret_replicas.rs` — `Secret<Open>` has no `replicas`
- `empty_apply.rs` — `Stack<Empty>` has no `apply`
- `applied_add.rs` — `Stack<Applied>` has no `add`

`.replicas(n)` is legal on `Postgres<Open>`, `Mysql<Open>`, and `Redis<Open>` (local / Tofu docker HA). Aws still rejects `replicas > 1` at validate time. `secret` is state-only.
