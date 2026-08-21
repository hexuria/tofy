# Public API

The written source is Rust. Builders are typestate: every builder is `Foo<S>` with `_state: PhantomData<S>` and a zero-sized state type. Methods that are illegal in a state **do not exist** on that impl (not `#[deprecated]`, not a runtime error).

`use tofy::prelude::*;` exports `postgres`, `redis`, `bucket`, `stack`, `Size`, `Bind`, and the `main` macro. `#[tofy::main]` still wraps `fn main`.

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
| `Stack<Empty>` | `add(resource) -> Stack<NonEmpty>` | `plan`, `apply`, `output`, `run` |
| `Stack<NonEmpty>` | `add -> Stack<NonEmpty>`, `plan(self)`, `apply(self) -> Stack<Applied>` | `output`, `run` |
| `Stack<Applied>` | `output`, `run` | `add`, a second `apply` that mutates the graph |

`.apply()` on `Stack<NonEmpty>` calls `engine::apply` and only then returns `Stack<Applied>`. `cargo run -p infra` with no verb still applies.

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

`TOFY_APPDB_URI` is the host loopback URI for the laptop. A sibling container on the private network uses `TOFY_APPDB_INTERNAL_URI` (`…@appdb:5432/…`).

The JSON IR (`Project` / `Resource` / `Kind` in `tofy-spec`) is what the engine consumes. `tofy apply --spec spec.json` applies that IR without compiling Rust. Humans write the Rust file, not yaml.
