# tofy

Infrastructure from code, applied like OpenTofu.

You declare the databases, caches, and buckets the app needs in `tofy.yaml`.
`tofy plan` diffs that against a state file. `tofy apply` writes OpenTofu JSON
and a compose file, then starts containers if Docker is on the machine.

This is the first MVP. Macros that emit the same spec from Rust come next.
The language is small on purpose: app-adjacent resources only. VPCs and IAM
stay in real Terraform.

## Commands

```bash
cargo install --path .
tofy init
tofy plan
tofy apply
tofy output
tofy destroy
```

`tofy emit` writes artifacts without touching running containers.

## Spec

```yaml
project: demo
backend: local
resources:
  - name: appdb
    type: postgres
    version: "16"
    port: 5433
```

Types in this MVP: `postgres`, `redis`, `bucket` (MinIO).

## What apply writes

- `.tofy/state.json` — last applied graph
- `.tofy/outputs.json` — URIs and passwords the process can load
- `.tofy/main.tf.json` — OpenTofu/Terraform JSON (docker provider)
- `docker-compose.yml` — the local equivalent

If Docker is missing, apply still writes those files. Then:

```bash
docker compose up -d
# or
cd .tofy && tofu init && tofu apply
```

## How this is not Shuttle

Shuttle's macros provision on Shuttle's AWS. tofy's spec is desired state.
The engine is OpenTofu or Docker on *your* machine. Same declaration shape,
Terraform-shaped plan/state/destroy.

## Repo

https://github.com/hexuria/tofy
