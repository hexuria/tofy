# Public API

The written source is Rust. Builders are typestate: every builder is `Foo<S>` with `_state: PhantomData<S>` and a zero-sized state type. Methods that are illegal in a state **do not exist** on that impl (not `#[deprecated]`, not a runtime error).

`use tofy::prelude::*;` exports `postgres`, `mysql`, `redis`, `bucket`, `stack`, `Size`, `Bind`, `Backend`, and the `main` macro. `#[tofy::main]` still wraps `fn main`.

## Resource builders

`postgres("appdb")` → `Postgres<Open>`  
`mysql("appmysql")` → `Mysql<Open>`  
`redis("cache")` → `Redis<Open>`  
`bucket("uploads")` → `Bucket<Open>`

`Open` stays `Open` after setters. Adding a resource to a stack **consumes** it (move). There is no `.build()`.

| Type | Methods on `Open` | Not on this type |
| --- | --- | --- |
| `Postgres<Open>` | `version`, `port`, `size`, `bind`, `replicas` | `apply`, `add` |
| `Mysql<Open>` | `version`, `port`, `size`, `bind`, `replicas` | `apply`, `add` |
| `Redis<Open>` | `version`, `port`, `size`, `bind`, `replicas` | `apply`, `add` |
| `Bucket<Open>` | `version`, `port`, `size`, `bind` | `replicas`, `apply`, `add` |

`size` takes `Size { Small, Medium, Large }`, not a loose string.  
`bind` takes `Bind::Localhost` (`127.0.0.1` on Docker backends) or `Bind::All` (`0.0.0.0`). On `Backend::Aws`, `Localhost` is SG ingress from the applying machine's public IPv4 `/32` (not loopback, not a silent `0.0.0.0/0`); `All` opens SG ingress to `0.0.0.0/0`. RDS is publicly reachable from that CIDR. ElastiCache has no public IP, so Redis stays VPC-only even with the same SG. Laptop Redis requires a VPN or an SSH/SSM tunnel (`scripts/redis-tunnel.sh`); ElastiCache will not get a public IP.

The local and Tofu docker backends accept `.replicas(n)` on postgres / mysql / redis. Replica 0 is published on the host port; further replicas are in-stack only (`name-2`, …). The host URI is replica 0. `Backend::Aws` still rejects `replicas > 1`. `bucket` stays 1 (`bucket has no HA`). Default examples stay at 1.

`bucket("uploads")` starts the object store, waits until it accepts connections, and creates the bucket named `uploads`. `TOFY_UPLOADS_BUCKET` is that name only after the bucket exists.

## Stack

`stack("demo")` → `Stack<Empty>`

| State | Methods | Not on this state |
| --- | --- | --- |
| `Stack<Empty>` | `backend(Backend)`, `tofu()`, `aws()`, `add(resource) -> Stack<NonEmpty>` | `plan`, `apply`, `output`, `run` |
| `Stack<NonEmpty>` | `add -> Stack<NonEmpty>`, `plan(self)`, `apply(self) -> Stack<Applied>` | `output`, `run`, `backend` |
| `Stack<Applied>` | `output`, `run` | `add`, a second `apply` that mutates the graph |

`.backend(Backend::Tofu)` (or `.tofu()`) and `.backend(Backend::Aws)` (or `.aws()`) are legal only on `Stack<Empty>`. Default is `Backend::Local`. `tofy apply` applies whichever backend the declared spec already has.

`.apply()` on `Stack<NonEmpty>` calls `engine::apply` and only then returns `Stack<Applied>`. `cargo run -p infra` with no verb still applies.

`tofy plan` (and `Stack::plan`) depend on the spec backend. Local: refresh live Docker containers against `.tofy/state.json`. Reality that diverged — not running, missing, wrong image / published port / bind / labels — shows as a create or update, with a reason. Apply heals that drift. Tofu / Aws: run the OpenTofu engine plan against `.tofy/main.tf.json` (mode `0600`). Missing tofu, or missing ambient AWS credentials on Aws, errors; it does not print `No changes.` Plan does not mark resources Applied. Secrets stay out of the printed plan.

`cargo run -- plan` (and destroy / output / run / emit) still work: `apply()` sees the CLI verb, performs that verb, and **exits without returning Applied**. The type is not a lie.

## Compile-fail cases

These do not compile. trybuild covers them under `crates/tofy/tests/fail/`.

```rust
bucket("x").replicas(2);             // no replicas on Bucket<Open>
stack("d").apply();                  // no apply on Stack<Empty>
stack("d").add(postgres("x")).apply().add(postgres("y")); // no add on Stack<Applied>
```

Illegal `Size` values fail at compile time (`Size::Huge` does not exist).

## Consume path

After apply, other languages **do not** import tofy.

- `tofy run -- <cmd>` injects `TOFY_*` and execs
- or read `.tofy/outputs.env` / `.tofy/outputs.json`
- Rust-only opt-in: crate `tofy-pg` (`pool_from_env` / `pool_from_outputs`) plus `Stack<Applied>::uri(&self, name)` which returns the host URI string. `#[tofy::main]` stays sync. Other languages stay on env.

`TOFY_APPDB_URI` is the host URI for the laptop. On local / Tofu that is loopback; a sibling container on the private network uses `TOFY_APPDB_INTERNAL_URI` (`…@appdb:5432/…`). Redis is the same shape with a password: local / Tofu `TOFY_CACHE_URI` is `redis://:<password>@127.0.0.1:…`. On `Backend::Aws`, `TOFY_APPDB_URI` is the RDS endpoint (reachable from the applying machine) and `TOFY_CACHE_URI` is `rediss://:<password>@<elasticache-host>:…` (TLS). That Redis URI is for in-VPC or VPN clients — ElastiCache is not reachable from the public internet. `TOFY_CACHE_URI` from apply remains the ElastiCache `rediss://` host (in-VPC). After tunneling, the laptop uses `127.0.0.1`, not that hostname, unless DNS/VPN already routes there. Optional helper: `scripts/redis-tunnel.sh` (not invoked by apply).

The JSON IR (`Project` / `Resource` / `Kind` / `Backend` in `tofy-spec`) is what the engine consumes. `tofy apply --spec spec.json` applies that IR without compiling Rust. Humans write the Rust file, not yaml.

`tofy import compose <file>` maps a **constrained** Docker Compose subset (official `postgres` / `mysql` / `redis` / `minio/minio` images, ports, `mem_limit`) onto that same JSON IR. It writes spec JSON (or stdout). It does not apply, does not auto-load yaml, and `--spec` still rejects `.yaml` / `.yml`. Unknown images fail; Compose env passwords are not copied into the spec.

When `backend` is `tofu`, apply and plan run the OpenTofu engine against an emitted docker-provider config under `.tofy/` (mode `0600` if it contains secrets). When `backend` is `aws`, the same commands run the OpenTofu engine against an AWS-provider config. Credentials are ambient (`AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY`, `AWS_PROFILE`, shared files). The user-facing commands are still `tofy apply` / `tofy plan` / `tofy destroy`. `examples/infra-tofu` uses `stack("demotofu")` and host ports 15433 / 16379 / 19000. `examples/infra-aws` uses `stack("demoaws")` and ports 25432 / 26379.

## AWS mapping

Language stays `postgres` / `mysql` / `redis` / `bucket`. No `.vpc()`, `.instanceClass()`, or `.multiAz()` on the builders. `.replicas(n)` exists on postgres / mysql / redis Open; Aws still rejects `replicas > 1`.

| kind | AWS resource | Small | Medium | Large |
| --- | --- | --- | --- | --- |
| `postgres` | RDS instance | `db.t4g.micro` | `db.t4g.small` | `db.t4g.medium` |
| `mysql` | RDS instance | `db.t4g.micro` | `db.t4g.small` | `db.t4g.medium` |
| `redis` | ElastiCache Redis (1 node) | `cache.t4g.micro` | `cache.t4g.small` | `cache.t4g.medium` |
| `bucket` | S3 | `STANDARD` | `STANDARD` | `STANDARD` |

Postgres / Mysql / Redis passwords are generated once and persisted like the local backend. After apply, `TOFY_*_HOST` / `TOFY_*_URI` come from the engine. Redis on Aws is ElastiCache with in-transit encryption, so `TOFY_CACHE_URI` is `rediss://:<password>@…` (TLS), not `redis://`. The bucket is IAM-less: `TOFY_UPLOADS_BUCKET`, `TOFY_UPLOADS_REGION`, `TOFY_UPLOADS_ENDPOINT`.

At plan / apply / emit, tofy discovers the applying machine's public IPv4 and emits a tofy-owned security group in the account default VPC. Ingress is postgres / mysql / redis from that `/32` when bind is `Localhost`. RDS is `publicly_accessible` so the postgres / mysql host URI works from that machine. ElastiCache has no public IP: the same SG does not make Redis reachable from a laptop. Reach laptop Redis with a VPN or `scripts/redis-tunnel.sh` (SSH or SSM local-forward); `TOFY_CACHE_URI` still names the in-VPC ElastiCache host. If the public IP cannot be determined, the command errors; it does not open `0.0.0.0/0`. The `/32` is persisted in state so a later plan from a new IP is an SG-rule update. `Bind::All` is the documented wider ingress (`0.0.0.0/0`); the default example stays `Localhost`. No `.vpc()`, `.securityGroup()`, `.cidr()`, or `.publiclyAccessible()` on the builders.
