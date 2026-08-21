# Plan

## Phase 1 — this PR

Rust typestate builders (`Foo<S>` + `PhantomData`) and `#[tofy::main]` are the written source. They emit JSON IR. The local engine plans and applies with Docker.

- Typestate: `Postgres<Open>` / `Redis<Open>` / `Bucket<Open>`, `Stack<Empty>` → `NonEmpty` → `Applied`
- Local Docker backend: plan vs `.tofy/state.json`, create / update / delete
- Private stack network; `.bind(Bind::Localhost | Bind::All)`
- `.size(Size::Small | Medium | Large)`. No `.replicas()` on any Open builder. IR field default 1; `replicas > 1` rejected (`local backend has no HA`)
- Host URIs vs `INTERNAL_*` DNS URIs; `tofy run` injects `TOFY_*`
- Secrets generated once, state mode `0600`, apply lock
- Postgres readiness wait; object store readiness wait; create the named bucket before Applied
- `Stack::apply()` applies (`engine::apply`) and only then returns `Applied`. Other CLI verbs exit without that type.
- Missing Docker emits artifacts and exits non-zero (not Applied)
- Required Docker CI: `ubuntu-latest`, `cargo test`, real apply, connect, prove bucket exists, destroy. **Fails if Docker is missing.**
- These docs: `README.md`, this file, `docs/api.md`
- No yaml auto-load. `--spec` is JSON IR only.

No AWS RDS, VPC, Multi-AZ, or “run tofu yourself.” OpenTofu is not the apply engine yet.

## Phase 2 — OpenTofu backend

A real OpenTofu backend: `tofu init` / `tofu apply` / `tofu destroy` against the emitted configuration.

- Docker provider first (same three kinds)
- Remote AWS only with ambient credentials already on the machine (env / profile). No credential minting in tofy.
- Do not add RDS, Multi-AZ, VPC, subnets, security groups, or load balancers in this phase
- Keep the IR stable. Size tokens map to instance class; the replicas field stays in IR for a later backend

## Phase 3 — drift and polish

- Drift: refresh live containers vs state, show a plan when reality diverged
- Surface lock / drift in CI (provision CI is already phase 1)
- Polish: errors, output formatting, docs

## Later

- Importers into the same IR (not a write path; not auto-loaded)
- More app-adjacent kinds
- Optional live `PgPool` after apply for Rust apps that want it. Do not default to Shuttle. The consume path for other languages stays env.
