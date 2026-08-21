# Plan

## Phase 1 — this PR

Rust is the written source. Builders and `#[tofy::main]` emit a language-agnostic spec (`Project` / `Resource` / `Kind` in `tofy-spec`). The engine plans against `.tofy/state.json` and applies with a local Docker backend.

- Public API: `postgres` / `redis` / `bucket` / `stack`, plus `Size` and `Bind`
- `cargo run` in `examples/infra` is apply
- CLI: `tofy --dir <path> plan|apply|destroy|output|run|emit`
- `tofy apply --spec spec.json` applies IR without compiling Rust
- Secrets generated once and persisted; plan redacts them
- Outputs as `TOFY_<RESOURCE>_<KEY>` in `.tofy/outputs.json` and `.tofy/outputs.env`
- Host URIs stay on `127.0.0.1`; in-stack URIs use the resource DNS name
- Each stack gets a private Docker network; resources resolve by name
- `.bind(Bind::Localhost | Bind::All)` for published ports (default `127.0.0.1`)
- `.size(Size::Small / Medium / Large)` → local memory/CPU; later instance class
- `.replicas(n)` on redis/bucket; postgres stays at 1 (local backend has no HA)
- Plan shows size / replica / bind changes as updates
- `tofy run -- <cmd>` injects those env vars
- State lock, state mode `0600`
- Wait until Postgres accepts connections
- Missing Docker: emit artifacts, exit non-zero, do not claim Applied
- Destroy tears down containers and the stack network, clears state (keeps emitted `main.tf.json`)

No AWS RDS, VPC, or Multi-AZ. OpenTofu is not the apply engine yet.

## Phase 2 — OpenTofu backend

A real OpenTofu backend, not a compose wrapper that happens to write `tf.json`.

- `tofu init` / `tofu apply` / `tofu destroy` against the emitted configuration
- Docker provider first (same three kinds, still local)
- Remote AWS only with ambient credentials already on the machine (env / profile). No credential minting in tofy.
- Do not add RDS, Multi-AZ, VPC, subnets, security groups, or load balancers
- Keep the IR stable so the Rust frontend does not change
- Size tokens map to instance class; replicas stay attributes, not new types

## Phase 3 — CI and drift

- CI workflow: `cargo test`, and a smoke apply when Docker is available
- Drift: refresh live containers vs state, show a plan when reality diverged
- Fail apply when another process holds the lock (already in phase 1) and surface lock / drift in CI
