# tofy

A Rust control language for infrastructure. You write typestate builders and `#[tofy::main]`. That is the source. Those emit a language-agnostic JSON resource spec. `apply` consumes the spec: plan against state, create / update / delete, write outputs.

A Node, Go, or other app does not import tofy. It reads `TOFY_*` environment variables (or `.tofy/outputs.json`). There is no yaml happy path.

```rust
use tofy::prelude::*;

#[tofy::main]
fn main() {
    let db = postgres("appdb")
        .version("16")
        .port(5433)
        .size(Size::Small)
        .bind(Bind::Localhost);
    let cache = redis("cache");
    let files = bucket("uploads");
    stack("demo").add(db).add(cache).add(files).apply();
}
```

`postgres()` is a declaration, not a live connection. Builders are `Foo<S>` with `PhantomData` — illegal methods do not exist on that impl. `stack("demo")` cannot `apply` until you `add` a resource. After `apply()` you cannot `add` again. See [docs/api.md](docs/api.md).

`.apply()` on the stack is apply. `cargo run -p infra` is apply.

## What apply does

1. Plans the declared stack against `.tofy/state.json`.
2. Creates a private Docker network for the stack. Resources resolve each other by name (`appdb`, `cache`, `uploads`).
3. Generates secrets once (passwords, object-store keys) and persists them in state. They are never re-derived as `tofy-{project}-{name}`.
4. Starts containers with Docker. Published ports default to `127.0.0.1`. Apply waits until Postgres, Redis, and the object store accept connections (Redis AUTH, named bucket created). A dead port is not Applied.
5. Writes `.tofy/outputs.env` and `.tofy/outputs.json` (mode `0600`). Host consumers (`tofy run` on the laptop) get `127.0.0.1` URIs. Sibling containers on the stack network use `INTERNAL_*` keys (`postgres://…@appdb:5432/…`).
6. `tofy run -- <cmd>` injects those env vars and execs. Apps do not depend on dotenv.

Apply writes `spec.json` (IR, no secrets). It does **not** write `docker-compose.yml` or `main.tf.json` — those would embed live passwords.

If Docker is missing: spec JSON may be written, the process exits non-zero, and apply does **not** claim Applied. Destroy also requires Docker: it errors and leaves state alone. It does not print Destroyed.

OpenTofu is the phase 2 **engine**, not a command you run yourself. Phase 1 does not print “go run `tofu apply`.”

## Commands

```bash
cargo install --path crates/tofy

# public path — cargo run in the infra crate is apply
cargo run -p infra -- --dir examples/infra
cargo run -p infra -- --dir examples/infra plan
cargo run -p infra -- --dir examples/infra destroy

# CLI pointed at that crate
tofy --dir examples/infra plan
tofy --dir examples/infra apply
tofy --dir examples/infra output
tofy --dir examples/infra run -- node app.js
tofy --dir examples/infra emit
tofy --dir examples/infra destroy

# apply an already-emitted spec JSON (no Rust on that machine)
tofy --dir . apply --spec spec.json
```

`tofy plan` redacts passwords. `tofy output` prints non-secret keys; `--json` dumps the local outputs file. Destroy tears down containers and clears state. If Docker is missing, destroy errors and does not clear state.

## Env vars

After apply, names are `TOFY_<RESOURCE>_<KEY>`:

| Resource | Keys |
| --- | --- |
| stack | `TOFY_NETWORK` (`tofy-demo`) |
| `appdb` (postgres) | `TOFY_APPDB_URI`, `TOFY_APPDB_PASSWORD`, `TOFY_APPDB_USER`, `TOFY_APPDB_DATABASE`, `TOFY_APPDB_PORT`, `TOFY_APPDB_HOST`, plus `TOFY_APPDB_INTERNAL_*` |
| `cache` (redis) | `TOFY_CACHE_URI` (`redis://:<password>@127.0.0.1:…`), `TOFY_CACHE_PASSWORD`, `TOFY_CACHE_PORT`, `TOFY_CACHE_HOST`, plus `TOFY_CACHE_INTERNAL_*` |
| `uploads` (bucket) | `TOFY_UPLOADS_ENDPOINT`, `TOFY_UPLOADS_ACCESS_KEY`, `TOFY_UPLOADS_SECRET_KEY`, `TOFY_UPLOADS_BUCKET`, `TOFY_UPLOADS_PORT`, plus `TOFY_UPLOADS_INTERNAL_*` |

**Host vs in-stack.** `tofy run` and processes on the laptop use loopback (`TOFY_APPDB_URI=postgres://…@127.0.0.1:5433/…`). Another container on the private network uses the resource DNS name and the container port (`TOFY_APPDB_INTERNAL_URI=postgres://…@appdb:5432/…`).

`.tofy/` is gitignored. Do not commit `state.json`, `outputs.env`, or secrets.

## Size and bind

Attributes, not new resource types. Language stays `postgres`, `redis`, `bucket`.

| size | local Docker | later OpenTofu instance class |
| --- | --- | --- |
| `small` (default) | 256MiB, 0.25 CPU | `small` |
| `medium` | 512MiB, 0.50 CPU | `medium` |
| `large` | 1GiB, 1.00 CPU | `large` |

The local backend has no HA. There is no `.replicas()` on `postgres`, `redis`, or `bucket`. The IR field exists (default 1) for a later backend. `replicas > 1` in JSON is rejected: `local backend has no HA`. Plan treats size and bind changes as updates.

`.bind(Bind::Localhost)` (default) or `.bind(Bind::All)` (`0.0.0.0`) is who can reach the **published** port. In-stack traffic still uses the private network. Redis always has `requirepass` (password in `TOFY_CACHE_PASSWORD` / URI) so `Bind::All` is not an open unauthenticated Redis.

**Out of scope (real Terraform / OpenTofu):** VPCs, subnets, security groups, load balancers, IAM.

## CI

Required Docker provision on GitHub-hosted `ubuntu-latest`. Docker is not disabled.

1. `cargo test --workspace`
2. `cargo run -p infra -- --dir examples/infra apply` — must exit 0, state `applied`
3. Health checks: containers running, Postgres accepts connections, Redis PING, named object-store bucket exists (not just TCP)
4. `tofy run` can read `TOFY_APPDB_URI`
5. `tofy destroy` and containers plus the stack network are gone

If Docker is missing, the job **fails**. It does not skip. It does not treat “emitted compose, exit 1” as success.

## What this is not

**Not Shuttle.** Shuttle's macros provision on Shuttle's cloud. tofy declarations are desired state. You apply them on your machine (Docker today, OpenTofu later). The process that runs your app only reads env.

**Not Compose.** Compose is a container file format. tofy is a control language plus a planner. The local backend starts containers with Docker. Apply does not write a compose file.

**Not just OpenTofu.** OpenTofu is a later backend ([PLAN.md](PLAN.md) phase 2). The product is the Rust frontend and the IR. You do not run `tofu apply` yourself in phase 1.

## Repo

https://github.com/hexuria/tofy

Apache-2.0
