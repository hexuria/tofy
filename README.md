# tofy

A Rust control language for infrastructure. You write declarations with macros and builders. Those emit a language-agnostic resource spec (JSON IR). The engine plans and applies that spec the way OpenTofu plans and applies Terraform: diff against state, create / update / delete, write outputs.

Phase 1 applies locally with Docker. Phase 2 is a real OpenTofu backend. The Rust file is what humans write. The JSON is what the engine consumes. A Node or Go app never imports tofy; it reads env vars or `.tofy/outputs.json`.

```rust
use tofy::prelude::*;

#[tofy::main]
fn main() {
    let db = postgres("appdb")
        .version("16")
        .port(5433)
        .size(Size::Small);
    let cache = redis("cache");
    let files = bucket("uploads");
    stack("demo").add(db).add(cache).add(files);
}
```

`postgres()` returns a declaration, not a live connection. `cargo run` in the infra crate is apply.

Each stack gets a private Docker network (`tofy-demo` here). Resources resolve each other by name (`appdb`, `cache`, `uploads`). You do not declare a network. Published ports default to `127.0.0.1`; use `.bind(Bind::All)` for `0.0.0.0`.

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
| stack | `TOFY_NETWORK` (`tofy-demo`) |
| `appdb` (postgres) | `TOFY_APPDB_URI`, `TOFY_APPDB_PASSWORD`, `TOFY_APPDB_USER`, `TOFY_APPDB_DATABASE`, `TOFY_APPDB_PORT`, `TOFY_APPDB_HOST`, plus `TOFY_APPDB_INTERNAL_*` |
| `cache` (redis) | `TOFY_CACHE_URI`, `TOFY_CACHE_PORT`, `TOFY_CACHE_HOST`, plus `TOFY_CACHE_INTERNAL_*` |
| `uploads` (bucket) | `TOFY_UPLOADS_ENDPOINT`, `TOFY_UPLOADS_ACCESS_KEY`, `TOFY_UPLOADS_SECRET_KEY`, `TOFY_UPLOADS_BUCKET`, `TOFY_UPLOADS_PORT`, plus `TOFY_UPLOADS_INTERNAL_*` |

`tofy run` on the host uses loopback URIs (`postgres://…@127.0.0.1:5433/…`). A sibling container on the stack network uses the DNS name and the container port (`postgres://…@appdb:5432/…`) from the `INTERNAL_*` keys.

## Size and replicas

`.size(Size::Small | Medium | Large)` is an attribute, not a new resource type. Local Docker maps it to memory/CPU. A later OpenTofu backend maps the same token to instance class.

| size | local Docker | later OpenTofu instance class |
| --- | --- | --- |
| `small` (default) | 256MiB, 0.25 CPU | `small` |
| `medium` | 512MiB, 0.50 CPU | `medium` |
| `large` | 1GiB, 1.00 CPU | `large` |

`.replicas(n)` is allowed on `redis` and `bucket`. Local postgres stays at 1; `replicas > 1` fails with `local backend has no HA`. Plan treats size, replica, and bind changes as updates. Extra local replicas share the resource DNS name; only replica 1 is published to the host.

Secrets (passwords, keys, URIs that embed them) are generated once, stored in `.tofy/state.json` (mode `0600`), and reused on the next apply. They are never re-derived as `tofy-{project}-{name}`.

`.tofy/` is gitignored. Do not commit `state.json` or `outputs.env`.

## What this is not

**Not Shuttle.** Shuttle's macros provision on Shuttle's cloud. tofy declarations are desired state. You apply them on your machine (Docker today, OpenTofu later). The process that runs your app only reads env.

**Not Compose.** Compose is a container file format. tofy is a control language plus a planner. The local backend may start containers with Docker, and it may write a compose file as an artifact, but you do not write that file and you do not treat tofy as `docker compose` with extra steps.

**Not just OpenTofu.** OpenTofu is a later backend (see [PLAN.md](PLAN.md)). The product is the Rust frontend and the IR. Phase 1 does not shell out to `tofu apply`.

## Language

Small on purpose: `postgres`, `redis`, `bucket`. App-adjacent resources only.

**Out of scope (use real Terraform / OpenTofu):** VPCs, subnets, security groups, load balancers, IAM. This language does not grow those types. The private Docker network is for in-stack DNS, not a VPC.

YAML is an importer to the same IR (`tofy apply --spec tofy.yaml`), not the happy path.

## Repo

https://github.com/hexuria/tofy

Apache-2.0
