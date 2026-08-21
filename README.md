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

That default is the local Docker engine. The same resource lines select an OpenTofu engine with a typestate-legal selector on `Stack<Empty>`:

```rust
stack("demotofu").backend(Backend::Tofu).add(db).add(cache).add(files).apply();
stack("demoaws").backend(Backend::Aws).add(db).add(cache).add(files).apply();
```

`examples/infra` stays `stack("demo")` on 5433 / 6379 / 9000. `examples/infra-tofu` is `stack("demotofu")` with host ports 15433 / 16379 / 19000 so both can run on one machine. `examples/infra-aws` is `stack("demoaws")` with ports 25432 / 26379 (`Backend::Aws`).

`postgres()` is a declaration, not a live connection. Builders are `Foo<S>` with `PhantomData` — illegal methods do not exist on that impl. `stack("demo")` cannot `apply` until you `add` a resource. After `apply()` you cannot `add` again. See [docs/api.md](docs/api.md).

`.apply()` on the stack is apply. `cargo run -p infra` is apply. `tofy apply` applies whatever backend the declared spec already has. OpenTofu is an optional engine underneath that path — not a command you run yourself.

## What apply does

1. Plans the declared stack against `.tofy/state.json` **and live containers** (running, image, published port/bind). A `docker stop` or a remapped publish is a change.
2. Creates a private Docker network for the stack. Resources resolve each other by name (`appdb`, `cache`, `uploads`).
3. Generates secrets once (passwords, object-store keys) and persists them in state. They are never re-derived as `tofy-{project}-{name}`.
4. Starts resources. The local backend uses Docker directly. The Tofu backend emits a docker-provider config and runs the OpenTofu engine. The Aws backend emits an AWS-provider config and runs the same engine against RDS, ElastiCache Redis, and S3. Published ports on Docker backends default to `127.0.0.1`. Local / Tofu apply waits until Postgres, Redis, and the object store accept connections (Redis AUTH, named bucket created). A dead port is not Applied.
5. Writes `.tofy/outputs.env` and `.tofy/outputs.json` (mode `0600`). Host consumers (`tofy run` on the laptop) get `127.0.0.1` URIs. Sibling containers on the stack network use `INTERNAL_*` keys (`postgres://…@appdb:5432/…`).
6. `tofy run -- <cmd>` injects those env vars and execs. Apps do not depend on dotenv.

Apply writes `spec.json` (IR, no secrets). The local backend does **not** write `docker-compose.yml` or `main.tf.json` — those would embed live passwords as a world-readable default. The Tofu and Aws backends write `.tofy/main.tf.json` (mode `0600`, gitignored) because the OpenTofu config must contain secrets, plus tofu state under `.tofy/`.

If Docker is missing on the local backend: spec JSON may be written, the process exits non-zero, and apply does **not** claim Applied. Destroy also requires Docker: it errors and leaves state alone. It does not print Destroyed.

If the spec backend is Tofu or Aws and the OpenTofu engine is missing: plan, apply, and destroy error; plan does not print `No changes.`; apply / destroy do not print Applied / Destroyed; destroy leaves state alone. The message is that the OpenTofu engine is required for this backend — not a prompt to run OpenTofu yourself.

If the spec backend is Aws and ambient AWS credentials are not already on the machine (`AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY`, `AWS_PROFILE`, or the shared credentials file): plan, apply, and destroy error the same way. tofy does not mint, prompt, store, or commit credentials, and there is no AWS login command.

## Commands

```bash
cargo install --path crates/tofy

# public path — cargo run in the infra crate is apply (local Docker)
cargo run -p infra -- --dir examples/infra
cargo run -p infra -- --dir examples/infra plan
cargo run -p infra -- --dir examples/infra destroy

# same command, OpenTofu docker-provider engine (Backend::Tofu in the crate)
cargo run -p infra-tofu -- --dir examples/infra-tofu
cargo run -p infra-tofu -- --dir examples/infra-tofu plan

# same command, OpenTofu AWS-provider engine (Backend::Aws; needs ambient AWS credentials)
cargo run -p infra-aws -- --dir examples/infra-aws
cargo run -p infra-aws -- --dir examples/infra-aws plan
cargo run -p infra-aws -- --dir examples/infra-aws emit

# CLI pointed at that crate
tofy --dir examples/infra plan
tofy --dir examples/infra-tofu plan
tofy --dir examples/infra apply
tofy --dir examples/infra output
tofy --dir examples/infra run -- node app.js
tofy --dir examples/infra emit
tofy --dir examples/infra destroy

# apply an already-emitted spec JSON (no Rust on that machine)
tofy --dir . apply --spec spec.json
```

`tofy plan` on the local backend refreshes live containers against `.tofy/state.json` and redacts passwords. A stopped or remapped container is a change, with a reason (`not running`, `port changed`). On `Backend::Tofu` and `Backend::Aws`, `tofy plan` runs the OpenTofu engine plan against the emitted 0600 `.tofy/main.tf.json` (init if needed) and prints that plan, with secrets redacted. Missing tofu is an error — it does not print `No changes.` as if it planned. Missing ambient AWS credentials on `Backend::Aws` is the same kind of error. Plan does not mark resources Applied. `tofy output` prints non-secret keys; `--json` dumps the local outputs file. Destroy tears down resources and clears state. If Docker is missing (local backend), the OpenTofu engine is missing (Tofu / Aws), or AWS credentials are missing (Aws), destroy errors and does not clear state. A second apply or destroy in the same directory while one is running is `Locked` (exclusive `flock`; a crash does not leave a permanent lock).

## Env vars

After apply, names are `TOFY_<RESOURCE>_<KEY>`:

| Resource | Keys |
| --- | --- |
| stack | `TOFY_NETWORK` (`tofy-demo`) |
| `appdb` (postgres) | `TOFY_APPDB_URI`, `TOFY_APPDB_PASSWORD`, `TOFY_APPDB_USER`, `TOFY_APPDB_DATABASE`, `TOFY_APPDB_PORT`, `TOFY_APPDB_HOST`, plus `TOFY_APPDB_INTERNAL_*` |
| `cache` (redis) | `TOFY_CACHE_URI` (`redis://:<password>@127.0.0.1:…`), `TOFY_CACHE_PASSWORD`, `TOFY_CACHE_PORT`, `TOFY_CACHE_HOST`, plus `TOFY_CACHE_INTERNAL_*` |
| `uploads` (bucket) | Local / Tofu: `TOFY_UPLOADS_ENDPOINT`, `TOFY_UPLOADS_ACCESS_KEY`, `TOFY_UPLOADS_SECRET_KEY`, `TOFY_UPLOADS_BUCKET`, `TOFY_UPLOADS_PORT`, plus `TOFY_UPLOADS_INTERNAL_*`. Aws: `TOFY_UPLOADS_BUCKET`, `TOFY_UPLOADS_REGION`, `TOFY_UPLOADS_ENDPOINT` (no minted keys) |

**Host vs in-stack.** `tofy run` and processes on the laptop use loopback (`TOFY_APPDB_URI=postgres://…@127.0.0.1:5433/…`). Another container on the private network uses the resource DNS name and the container port (`TOFY_APPDB_INTERNAL_URI=postgres://…@appdb:5432/…`).

``.tofy/` is gitignored. Do not commit `state.json`, `outputs.env`, `main.tf.json`, tofu state, or secrets.

## Size and bind

Attributes, not new resource types. Language stays `postgres`, `redis`, `bucket`.

| size | local Docker and OpenTofu docker provider | AWS (`Backend::Aws`) |
| --- | --- | --- |
| `small` (default) | 256MiB, 0.25 CPU | RDS `db.t4g.micro`, ElastiCache `cache.t4g.micro`, S3 `STANDARD` |
| `medium` | 512MiB, 0.50 CPU | RDS `db.t4g.small`, ElastiCache `cache.t4g.small`, S3 `STANDARD` |
| `large` | 1GiB, 1.00 CPU | RDS `db.t4g.medium`, ElastiCache `cache.t4g.medium`, S3 `STANDARD` |

The local backend has no HA. There is no `.replicas()` on `postgres`, `redis`, or `bucket`. The IR field exists (default 1) for a later backend. `replicas > 1` in JSON is rejected: `local backend has no HA`. Plan treats size and bind changes as updates.

`.bind(Bind::Localhost)` (default) or `.bind(Bind::All)` (`0.0.0.0`) is who can reach the **published** port. In-stack traffic still uses the private network. Redis always has `requirepass` (password in `TOFY_CACHE_PASSWORD` / URI) so `Bind::All` is not an open unauthenticated Redis.

The Aws backend maps the same tokens to RDS / ElastiCache instance class and S3 `STANDARD`. It does not add instance-class, replica, or networking methods to the builders.

## CI

Three required jobs on GitHub-hosted `ubuntu-latest`. rustc is pinned to 1.83 (`rust-toolchain.toml`) so trybuild stays honest.

**Local Docker** (`scripts/ci-smoke.sh`): `cargo test --workspace`, then `cargo run -p infra` (default `Backend::Local`). Docker is not disabled. Missing Docker **fails**.

**OpenTofu docker engine** (`scripts/ci-smoke-tofu.sh`): installs OpenTofu, then `cargo run -p infra-tofu` (`stack("demotofu").backend(Backend::Tofu)…` on 15433 / 16379 / 19000). `tofy plan` must print the OpenTofu engine plan, not only the house `Plan:` / `+ create` format. Missing Docker or missing tofu **fails**.

**AWS OpenTofu config** (`scripts/ci-smoke-aws.sh`): unit tests, emit `examples/infra-aws` (`stack("demoaws").backend(Backend::Aws)…`), `tofu init` + `tofu validate`, and prove missing-creds apply / plan do not claim Applied or `No changes.`. Does **not** live-apply AWS. Missing tofu **fails**. Skip-as-success would be a lie.

Both jobs:

1. Apply must exit 0, state `applied`
2. Health checks: containers running, Postgres accepts connections, Redis PING, named object-store bucket exists (not just TCP)
3. `tofy run` can read `TOFY_APPDB_URI`
4. After apply, stop a container: `tofy plan` must not print `No changes.`; apply heals; probes still pass
5. `tofy destroy` and containers plus the stack network are gone

`cargo test` holds the apply lock and asserts a second apply/destroy is `Locked`. Skip-without-Docker or skip-without-tofu is a fail.

## What this is not

**Not Shuttle.** Shuttle's macros provision on Shuttle's cloud. tofy declarations are desired state. You apply them on your machine (Docker by default, OpenTofu when you select that backend). The process that runs your app only reads env.

**Not Compose.** Compose is a container file format. tofy is a control language plus a planner. The local backend starts containers with Docker. Apply does not write a compose file.

**Not a tofu CLI wrapper.** OpenTofu is an optional engine ([PLAN.md](PLAN.md)). The product is the Rust frontend and the IR. You run `tofy apply` / `tofy plan` / `tofy destroy`.

## Repo

https://github.com/hexuria/tofy

Apache-2.0
