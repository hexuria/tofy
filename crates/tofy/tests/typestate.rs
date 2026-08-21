//! Compile-fail typestate: illegal methods do not exist on that impl.
//!
//! These must not compile (see `fail/`):
//! - `postgres("x").replicas(2)`
//! - `redis("x").replicas(2)`
//! - `bucket("x").replicas(2)`
//! - `stack("d").apply()`
//! - `applied.add(db)`

#[test]
fn typestate_compile_fail() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/fail/postgres_replicas.rs");
    t.compile_fail("tests/fail/redis_replicas.rs");
    t.compile_fail("tests/fail/bucket_replicas.rs");
    t.compile_fail("tests/fail/empty_apply.rs");
    t.compile_fail("tests/fail/applied_add.rs");
}
