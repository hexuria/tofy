# tofy

A Rust control language for infrastructure. You write declarations with macros and builders. Those emit a language-agnostic resource spec (JSON IR). The engine plans and applies that spec the way OpenTofu plans and applies Terraform: diff against state, create / update / delete, write outputs.

Phase 1 applies locally with Docker. Phase 2 is a real OpenTofu backend. The Rust file is what humans write. The JSON is what the engine consumes. A Node or Go app never imports tofy; it reads env vars or `.tofy/outputs.json`.

```rust
use tofy::prelude::*;

#[tofy::main]
fn main() {
    let db = postgres("appdb").version("16").port(5433);
    let cache = redis("cache");
    let files = bucket("uploads");
    stack("demo").add(db).add(cache).add(files);
}
```

`postgres()` returns a declaration, not a live connection. `cargo run` in the infra crate is apply.

## Commands

```bash
cargo install --path crates/tofy

# from the crate that calls stack():
cargo run                  # apply
cargo run -- plan
cargo run -- destroy

# or the CLI, pointed at that crate:
tofy --dir examples/infra plan
tofy --dir examples/infra apply
tofy --dir examples/infra output
tofy --dir examples/infra run -- node app.js
tofy --dir examples/infra emit
tofy --dir examples/infra destroy

# apply an already-emitted spec (no Rust on that machine)
tofy --dir . apply --spec spec.json
```

`tofy plan` redacts passwords. `tofy output` prints non-secret keys; `--json` dumps everything from the local outputs file. `tofy run -- <cmd>` injects those env vars and execs — apps do not depend on dotenv.

## Env vars

After apply, `.tofy/outputs.env` and `.tofy/outputs.json` use `TOFY_<RESOURCE>_<KEY>`:

| Resource | Keys |
| --- | --- |
| `appdb` (postgres) | `TOFY_APPDB_URI`, `TOFY_APPDB_PASSWORD`, `TOFY_APPDB_USER`, `TOFY_APPDB_DATABASE`, `TOFY_APPDB_PORT`, `TOFY_APPDB_HOST` |
| `cache` (redis) | `TOFY_CACHE_URI`, `TOFY_CACHE_PORT`, `TOFY_CACHE_HOST` |
| `uploads` (bucket) | `TOFY_UPLOADS_ENDPOINT`, `TOFY_UPLOADS_ACCESS_KEY`, `TOFY_UPLOADS_SECRET_KEY`, `TOFY_UPLOADS_BUCKET`, `TOFY_UPLOADS_PORT` |

Secrets (passwords, keys, URIs that embed them) are generated once, stored in `.tofy/state.json` (mode `0600`), and reused on the next apply. They are never re-derived as `tofy-{project}-{name}`.

`.tofy/` is gitignored. Do not commit `state.json` or `outputs.env`.

## What this is not

**Not Shuttle.** Shuttle's macros provision on Shuttle's cloud. tofy declarations are desired state. You apply them on your machine (Docker today, OpenTofu later). The process that runs your app only reads env.

**Not Compose.** Compose is a container file format. tofy is a control language plus a planner. The local backend may start containers with Docker, and it may write a compose file as an artifact, but you do not write that file and you do not treat tofy as `docker compose` with extra steps.

**Not just OpenTofu.** OpenTofu is a later backend (see [PLAN.md](PLAN.md)). The product is the Rust frontend and the IR. Phase 1 does not shell out to `tofu apply`.

## Language

Small on purpose: `postgres`, `redis`, `bucket`. App-adjacent resources only. VPCs and IAM stay in real Terraform / OpenTofu.

YAML is an importer to the same IR (`tofy apply --spec tofy.yaml`), not the happy path.

## Repo

https://github.com/hexuria/tofy

Apache-2.0
