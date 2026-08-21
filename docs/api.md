# Public API

The written source is Rust. Builders are typestate: every builder is `Foo<S>` with `_state: PhantomData<S>` and a zero-sized state type. Methods that are illegal in a state **do not exist** on that impl (not `#[deprecated]`, not a runtime error).

`use tofy::prelude::*;` exports `postgres`, `redis`, `bucket`, `stack`, `Size`, `Bind`, `Backend`, and the `main` macro. `#[tofy::main]` still wraps `fn main`.

## Resource builders

`postgres("appdb")` → `Postgres<Open>`  
`redis("cache")` → `Redis<Open>`  
`bucket("uploads")` → `Bucket<Open>`

`Open` stays `Open` after setters. Adding a resource to a stack **consumes** it (move). There is no `.build()`.

| Type | Methods on `Open` | Not on this type |
| --- | --- | --- |
| `Postgres<Open>` | `version`, `port`, `size`, `bind` | `replicas`, `apply`, `add` |
| `Redis<Open>` | `version`, `port`, `size`, `bind` | `replicas`, `apply`, `add` |
| `Bucket<Open>` | `version`, `port`, `size`, `bind` | `replicas`, `apply`, `add` |

`size` takes `Size { Small, Medium, Large }`, not a loose string.  
`bind` takes `Bind::Localhost` (`127.0.0.1`) or `Bind::All` (`0.0.0.0`).

The local backend has no HA. `replicas` is not a method on any Open builder. If the IR has `replicas > 1` on any kind, apply fails with `local backend has no HA`. The IR field stays (default 1) for a later backend.

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
postgres("x").replicas(2);           // no replicas on Postgres<Open>
redis("x").replicas(2);              // no replicas on Redis<Open>
bucket("x").replicas(2);             // no replicas on Bucket<Open>
stack("d").apply();                  // no apply on Stack<Empty>
stack("d").add(postgres("x")).apply().add(postgres("y")); // no add on Stack<Applied>
```

Illegal `Size` values fail at compile time (`Size::Huge` does not exist).

## Consume path

After apply, other languages **do not** import tofy.

- `tofy run -- <cmd>` injects `TOFY_*` and execs
- or read `.tofy/outputs.env` / `.tofy/outputs.json`

`TOFY_APPDB_URI` is the host loopback URI for the laptop. A sibling container on the private network uses `TOFY_APPDB_INTERNAL_URI` (`…@appdb:5432/…`). Redis is the same shape with a password: `TOFY_CACHE_URI` is `redis://:<password>@127.0.0.1:…` and `TOFY_CACHE_PASSWORD` is the secret.

The JSON IR (`Project` / `Resource` / `Kind` / `Backend` in `tofy-spec`) is what the engine consumes. `tofy apply --spec spec.json` applies that IR without compiling Rust. Humans write the Rust file, not yaml.

When `backend` is `tofu`, apply and plan run the OpenTofu engine against an emitted docker-provider config under `.tofy/` (mode `0600` if it contains secrets). When `backend` is `aws`, the same commands run the OpenTofu engine against an AWS-provider config. Credentials are ambient (`AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY`, `AWS_PROFILE`, shared files). The user-facing commands are still `tofy apply` / `tofy plan` / `tofy destroy`. `examples/infra-tofu` uses `stack("demotofu")` and host ports 15433 / 16379 / 19000. `examples/infra-aws` uses `stack("demoaws")` and ports 25432 / 26379.

## AWS mapping

Language stays `postgres` / `redis` / `bucket`. No `.vpc()`, `.instanceClass()`, `.multiAz()`, or `.replicas()` on the builders.

| kind | AWS resource | Small | Medium | Large |
| --- | --- | --- | --- | --- |
| `postgres` | RDS instance | `db.t4g.micro` | `db.t4g.small` | `db.t4g.medium` |
| `redis` | ElastiCache Redis (1 node) | `cache.t4g.micro` | `cache.t4g.small` | `cache.t4g.medium` |
| `bucket` | S3 | `STANDARD` | `STANDARD` | `STANDARD` |

Postgres / Redis passwords are generated once and persisted like the local backend. After apply, `TOFY_*_HOST` / `TOFY_*_URI` come from the engine. The bucket is IAM-less: `TOFY_UPLOADS_BUCKET`, `TOFY_UPLOADS_REGION`, `TOFY_UPLOADS_ENDPOINT`.
