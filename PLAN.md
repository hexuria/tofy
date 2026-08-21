# Plan

## Phase 1 — merged

Rust typestate builders (`Foo<S>` + `PhantomData`) and `#[tofy::main]` are the written source. They emit JSON IR. The local engine plans and applies with Docker.

- Typestate: `Postgres<Open>` / `Redis<Open>` / `Bucket<Open>`, `Stack<Empty>` → `NonEmpty` → `Applied`
- Local Docker backend: plan vs `.tofy/state.json`, create / update / delete
- Private stack network; `.bind(Bind::Localhost | Bind::All)`
- `.size(Size::Small | Medium | Large)`. No `.replicas()` on any Open builder. IR field default 1; `replicas > 1` rejected (`local backend has no HA`)
- Host URIs vs `INTERNAL_*` DNS URIs; `tofy run` injects `TOFY_*`
- Secrets generated once (postgres, redis, object-store keys), state/outputs mode `0600`, apply lock
- Apply does not write secret-bearing `docker-compose.yml` / `main.tf.json` as a world-readable default
- Postgres / Redis / object-store readiness wait; create the named bucket before Applied
- Redis `requirepass`; `TOFY_CACHE_URI` / `TOFY_CACHE_PASSWORD`
- `Stack::apply()` applies (`engine::apply`) and only then returns `Applied`. Other CLI verbs exit without that type.
- Missing Docker: apply exits non-zero (not Applied). Destroy errors and leaves state alone (does not print Destroyed).
- Required Docker CI: `ubuntu-latest`, `cargo test`, real apply, connect, prove bucket exists, destroy. **Fails if Docker is missing.**
- These docs: `README.md`, this file, `docs/api.md`
- No yaml auto-load. `--spec` is JSON IR only.

No AWS RDS, VPC, Multi-AZ, or “run tofu yourself.” Default apply stays the local Docker engine.

## Phase 2 — merged

A real OpenTofu backend. `Backend::Tofu` is no longer a dead enum. When the spec backend is Tofu, `tofy apply` / `tofy destroy` run the OpenTofu engine (`tofu init` / apply / destroy) against an emitted docker-provider configuration.

- Selector on `Stack<Empty>`: `.backend(Backend::Tofu)` (and `.tofu()`). Prelude exports `Backend`.
- Default remains `Backend::Local` so `stack("demo").add(...).apply()` is still docker.rs. Same postgres / redis / bucket lines.
- Docker provider (kreuzwerker/docker): private network, bind, size, volumes for postgres/minio, redis `--requirepass`, one container per resource (replicas stay 1)
- After tofu apply: wait until Postgres, Redis, and MinIO accept connections; create the named bucket; only then Applied
- Tofu state under `.tofy/` (gitignore). If the config contains secrets, `main.tf.json` is mode `0600`. tofy `.tofy/state.json` still holds generated secrets, ports, Applied status, outputs
- Missing OpenTofu engine: apply/destroy error, leave Applied/Destroyed unprinted, leave destroy state alone. Do not tell the user to run `tofu` themselves
- Failed tofu apply is not Applied
- File lock around apply/destroy
- Required CI job installs OpenTofu, applies `examples/infra-tofu`, probes connections and the named bucket, `tofy run` injects `TOFY_*`, then destroys. Missing Docker or missing tofu **fails**
- Phase 1 `scripts/ci-smoke.sh` stays the local Docker job and still requires Docker
- No RDS, Multi-AZ, VPC, subnets, security groups, load balancers, IAM, or autoscaler

## Phase 3 — merged

Drift, a real apply lock, and CI that fails if either is broken.

- `tofy plan` (and apply's plan) refresh live containers vs `.tofy/state.json`. A stopped, missing, or remapped container (image / published port / bind / labels) is a change, with a reason (`not running`, `port changed`). Passwords stay out of the plan text.
- Local apply heals: `ensure_running` for a stopped match, recreate if the container is gone or wrong. Tofu apply runs the OpenTofu engine (this phase still uses the docker provider; plan uses the same Docker inspect so a stopped container is not ignored).
- Exclusive `flock` on `.tofy/lock` for the lifetime of apply/destroy. A second apply in the same directory is `Locked`. Process death releases the lock (no stale pid-file).
- Both required smokes stay. After apply, stop a container: plan must not print `No changes.`, apply heals, probes still pass, destroy still works. `cargo test` holds the lock and asserts apply/destroy return `Locked`.
- Error strings stay honest: missing Docker / missing tofu / Locked do not print Applied or Destroyed.
- These docs: `README.md`, this file, `docs/api.md`

No AWS, no new kinds, no PgPool.

## Polish

- Honest `tofy plan` on `Backend::Tofu`: run the OpenTofu engine plan against the emitted 0600 `.tofy/main.tf.json` (init if needed). Print that plan. Redact secrets. Missing tofu errors and does not print `No changes.` as if it planned. Plan does not mark resources Applied. Local plan stays the live-refresh planner — do not replace it with tofu.
- `examples/infra-tofu` is `stack("demotofu")` with host ports 15433 / 16379 / 19000 so it can coexist with `examples/infra` (`demo` / 5433 / 6379 / 9000). Containers are `tofy-demotofu-*`. Same three resources and `.backend(Backend::Tofu)`.

## This PR — AWS backend

`Backend::Aws` is a real OpenTofu AWS-provider engine. Same postgres / redis / bucket lines. No VPC, RDS, or IAM methods on the builders.

- Selector on `Stack<Empty>`: `.backend(Backend::Aws)` (and `.aws()`). Prelude already exports `Backend`. Default remains `Backend::Local`. `Backend::Tofu` stays the docker-provider path.
- Apply / plan / destroy run the OpenTofu engine (`tofu init` / plan / apply / destroy) against emitted AWS-provider JSON under `.tofy/main.tf.json` (mode `0600`). User-facing commands stay `tofy apply` / `tofy plan` / `tofy destroy`.
- Mapping (language unchanged):

  | kind | AWS resource | Small | Medium | Large |
  | --- | --- | --- | --- | --- |
  | `postgres` | RDS (`aws_db_instance`) | `db.t4g.micro` | `db.t4g.small` | `db.t4g.medium` |
  | `redis` | ElastiCache Redis (1 node) | `cache.t4g.micro` | `cache.t4g.small` | `cache.t4g.medium` |
  | `bucket` | S3 | `STANDARD` | `STANDARD` | `STANDARD` |

- Replicas stay IR default 1. No `.replicas()`, `.vpc()`, `.instanceClass()`, `.multiAz()`.
- Networking is the account default VPC via OpenTofu data sources. tofy does not create a VPC and does not add VPC language.
- Credentials are ambient only (`AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY`, `AWS_PROFILE`, `AWS_REGION` / `AWS_DEFAULT_REGION`, shared config files). No minting, prompt, store, commit, or `tofy` AWS login.
- Missing tofu or missing ambient AWS credentials: plan / apply / destroy error. Do not print Applied or Destroyed. Do not tell the user to run tofu themselves.
- Secrets generated once (postgres / redis passwords) and persisted in `.tofy/state.json`. After apply, hosts come from OpenTofu outputs. S3 is IAM-less: bucket name + region + endpoint (no minted access keys). Plan redacts secrets.
- Required CI job: unit tests, emit `examples/infra-aws`, `tofu init` + `tofu validate`, prove missing-creds apply / plan do not claim Applied / `No changes.`. Does **not** live-apply AWS. Local and tofu-docker smokes stay required and unchanged.
- `examples/infra-aws` is `stack("demoaws")` with ports 25432 / 26379 so it does not collide with `demo` or `demotofu`.

## Later

- Importers into the same IR (not a write path; not auto-loaded)
- More app-adjacent kinds
- Optional live `PgPool` after apply for Rust apps that want it. Do not default to Shuttle. The consume path for other languages stays env
