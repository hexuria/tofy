//! Compile-fail typestate: illegal methods do not exist on that impl.
//!
//! These must not compile (see `fail/`):
//! - `bucket("x").replicas(2)`
//! - `secret("x").replicas(2)`
//! - `stack("d").apply()`
//! - `applied.add(db)`
//!
//! `.replicas(n)` is legal on Postgres / Mysql / Redis Open.

#[test]
fn typestate_compile_fail() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/fail/bucket_replicas.rs");
    t.compile_fail("tests/fail/secret_replicas.rs");
    t.compile_fail("tests/fail/empty_apply.rs");
    t.compile_fail("tests/fail/applied_add.rs");
}
