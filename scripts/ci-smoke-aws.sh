#!/usr/bin/env bash
# AWS-backend smoke: emit + tofu validate. Do not live-apply AWS.
# Missing tofu fails. Missing AWS credentials must not print Applied.
# The user-facing commands stay `tofy plan` / `tofy apply`. Do not tell the
# user to run tofu themselves.
# Stack is `demoaws` (ports 25432 / 26379) so it does not collide with
# examples/infra or examples/infra-tofu.
set -euo pipefail

ROOT="${TOFY_SMOKE_DIR:-examples/infra-aws}"
PKG="${TOFY_SMOKE_PKG:-infra-aws}"
BIN=(cargo run -q -p tofy -- --dir "$ROOT")
export TOFY_SMOKE_ROOT="$ROOT"
# Deterministic applier /32 for emit + validate. Not a live AWS allowlist.
export TOFY_APPLIER_CIDR="${TOFY_APPLIER_CIDR:-203.0.113.10/32}"

if ! command -v tofu >/dev/null 2>&1; then
  echo "OpenTofu engine is required for this backend; refusing to treat emit-only as success."
  exit 1
fi
tofu version

echo "== emit (public path: cargo run -p $PKG emit, Backend::Aws) =="
set +e
cargo run -p "$PKG" -- --dir "$ROOT" emit 2>&1 | tee /tmp/tofy-aws-emit.log
EMIT_EC=${PIPESTATUS[0]}
set -e
if [[ "$EMIT_EC" -ne 0 ]]; then
  echo "emit exited $EMIT_EC"
  exit "$EMIT_EC"
fi
if grep -q "Applied." /tmp/tofy-aws-emit.log; then
  echo "emit claimed Applied."
  exit 1
fi
if grep -qi "go run tofu" /tmp/tofy-aws-emit.log; then
  echo "emit told the user to run tofu"
  exit 1
fi

echo "== emitted AWS OpenTofu config is 0600 and not docker-provider =="
python3 - <<PY
import json, stat, sys
from pathlib import Path
root = Path("$ROOT") / ".tofy"
spec_path = root / "spec.json"
main = root / "main.tf.json"
if not spec_path.exists():
    sys.exit("emit did not write .tofy/spec.json")
if not main.exists():
    sys.exit("emit did not write .tofy/main.tf.json")
mode = stat.S_IMODE(main.stat().st_mode)
if mode != 0o600:
    sys.exit(f"main.tf.json mode={oct(mode)}, expected 0o600")
spec = json.loads(spec_path.read_text())
if spec.get("backend") != "aws":
    sys.exit(f"spec backend={spec.get('backend')!r}, expected aws")
tf = json.loads(main.read_text())
prov = (tf.get("terraform") or {}).get("required_providers") or {}
if "aws" not in prov:
    sys.exit("main.tf.json is missing the AWS provider")
if (prov.get("aws") or {}).get("source") != "hashicorp/aws":
    sys.exit(f"aws provider source={prov.get('aws')}")
if "docker" in prov:
    sys.exit("AWS backend must not emit the docker provider")
resource = tf.get("resource") or {}
if "docker_container" in resource:
    sys.exit("AWS backend must not emit docker_container")
if "aws_vpc" in resource:
    sys.exit("AWS backend must not create a VPC")
if "aws_subnet" in resource:
    sys.exit("AWS backend must not create a subnet")
if "aws_lb" in resource or "aws_lb_listener" in resource:
    sys.exit("AWS backend must not create a load balancer")
if any(k.startswith("aws_iam_") for k in resource):
    sys.exit("AWS backend must not create IAM resources")
if "aws_s3_bucket" not in resource:
    sys.exit("AWS backend must emit aws_s3_bucket for bucket")
if "aws_db_instance" not in resource:
    sys.exit("AWS backend must emit aws_db_instance for postgres")
if "aws_elasticache_replication_group" not in resource:
    sys.exit("AWS backend must emit aws_elasticache_replication_group for redis")
data = tf.get("data") or {}
if "aws_vpc" not in data:
    sys.exit("AWS backend must look up the account default VPC")
db = resource["aws_db_instance"]
first = next(iter(db.values()))
if first.get("instance_class") != "db.t4g.micro":
    sys.exit(f"small postgres must map to db.t4g.micro, got {first.get('instance_class')}")
if first.get("multi_az") is True:
    sys.exit("must not enable Multi-AZ")
if first.get("publicly_accessible") is not True:
    sys.exit("postgres must be publicly reachable so the host URI works from the applier")
sgs = first.get("vpc_security_group_ids") or []
if not any("aws_security_group.tofy" in str(s) for s in sgs):
    sys.exit(f"postgres must attach the tofy SG, got {sgs}")
if any("aws_security_group.default" in str(s) for s in sgs):
    sys.exit("must not attach only the account default SG")
if "aws_security_group" not in resource:
    sys.exit("AWS backend must emit a tofy-owned aws_security_group")
sg = (resource.get("aws_security_group") or {}).get("tofy") or {}
if not sg:
    sys.exit("aws_security_group.tofy is missing")
if "data.aws_vpc.default" not in str(sg.get("vpc_id")):
    sys.exit("tofy SG must live in the account default VPC")
ingress = resource.get("aws_vpc_security_group_ingress_rule") or {}
if not ingress:
    sys.exit("AWS backend must emit SG ingress rules")
for name, rule in ingress.items():
    cidr = rule.get("cidr_ipv4") or ""
    if cidr == "0.0.0.0/0":
        sys.exit(f"Localhost ingress for {name} must not be 0.0.0.0/0")
    if not str(cidr).endswith("/32"):
        sys.exit(f"Localhost ingress for {name} must be a /32, got {cidr!r}")
cache = resource.get("aws_elasticache_replication_group") or {}
cfirst = next(iter(cache.values()))
csgs = cfirst.get("security_group_ids") or []
if not any("aws_security_group.tofy" in str(s) for s in csgs):
    sys.exit(f"redis must attach the tofy SG, got {csgs}")
print("emit wrote AWS OpenTofu JSON mode 0600; default-VPC data; tofy SG /32; no docker/VPC/IAM resources")
PY

echo "== tofu init + validate (no AWS credentials, no apply) =="
tofu -chdir="$ROOT/.tofy" init -input=false -no-color
tofu -chdir="$ROOT/.tofy" validate -no-color

echo "== apply without AWS credentials must not claim Applied =="
EMPTY_HOME="$(mktemp -d)"
set +e
env -u AWS_ACCESS_KEY_ID -u AWS_SECRET_ACCESS_KEY -u AWS_SESSION_TOKEN \
  -u AWS_PROFILE -u AWS_SHARED_CREDENTIALS_FILE -u AWS_CONFIG_FILE \
  -u AWS_CONTAINER_CREDENTIALS_RELATIVE_URI -u AWS_CONTAINER_CREDENTIALS_FULL_URI \
  -u AWS_WEB_IDENTITY_TOKEN_FILE \
  HOME="$EMPTY_HOME" \
  cargo run -p "$PKG" -- --dir "$ROOT" apply 2>&1 | tee /tmp/tofy-aws-apply.log
APPLY_EC=${PIPESTATUS[0]}
set -e
if [[ "$APPLY_EC" -eq 0 ]]; then
  echo "apply without AWS credentials exited 0; that is a lie"
  exit 1
fi
if grep -q "Applied." /tmp/tofy-aws-apply.log; then
  echo "apply without AWS credentials claimed Applied."
  cat /tmp/tofy-aws-apply.log
  exit 1
fi
if grep -qi "Destroyed" /tmp/tofy-aws-apply.log; then
  echo "apply without AWS credentials printed Destroyed"
  exit 1
fi
if grep -qi "go run tofu" /tmp/tofy-aws-apply.log; then
  echo "apply told the user to run tofu"
  exit 1
fi
if ! grep -q "AWS credentials were not found" /tmp/tofy-aws-apply.log \
  && ! grep -q "OpenTofu engine is required" /tmp/tofy-aws-apply.log; then
  echo "apply did not explain missing AWS credentials or missing OpenTofu engine"
  cat /tmp/tofy-aws-apply.log
  exit 1
fi
python3 - <<PY
import json, sys
from pathlib import Path
state_path = Path("$ROOT") / ".tofy" / "state.json"
if state_path.exists():
    state = json.loads(state_path.read_text())
    for name, r in (state.get("resources") or {}).items():
        if r.get("status") == "applied":
            sys.exit(f"missing-creds apply marked {name} Applied")
print("missing-creds apply did not mark Applied")
PY

echo "== plan without AWS credentials must error (not No changes.) =="
set +e
env -u AWS_ACCESS_KEY_ID -u AWS_SECRET_ACCESS_KEY -u AWS_SESSION_TOKEN \
  -u AWS_PROFILE -u AWS_SHARED_CREDENTIALS_FILE -u AWS_CONFIG_FILE \
  -u AWS_CONTAINER_CREDENTIALS_RELATIVE_URI -u AWS_CONTAINER_CREDENTIALS_FULL_URI \
  -u AWS_WEB_IDENTITY_TOKEN_FILE \
  HOME="$EMPTY_HOME" \
  cargo run -p "$PKG" -- --dir "$ROOT" plan 2>&1 | tee /tmp/tofy-aws-plan.log
PLAN_EC=${PIPESTATUS[0]}
set -e
if [[ "$PLAN_EC" -eq 0 ]]; then
  echo "plan without AWS credentials exited 0"
  exit 1
fi
if grep -q "No changes." /tmp/tofy-aws-plan.log; then
  echo "plan without AWS credentials printed No changes."
  cat /tmp/tofy-aws-plan.log
  exit 1
fi
if grep -q "Applied." /tmp/tofy-aws-plan.log; then
  echo "plan claimed Applied."
  exit 1
fi
if grep -qi "go run tofu" /tmp/tofy-aws-plan.log; then
  echo "plan told the user to run tofu"
  exit 1
fi
if ! grep -q "AWS credentials were not found" /tmp/tofy-aws-plan.log \
  && ! grep -q "OpenTofu engine is required" /tmp/tofy-aws-plan.log; then
  echo "plan did not explain missing AWS credentials or missing OpenTofu engine"
  cat /tmp/tofy-aws-plan.log
  exit 1
fi

echo "== CLI emit also writes AWS config =="
"${BIN[@]}" emit >/tmp/tofy-aws-cli-emit.log
test -f "$ROOT/.tofy/main.tf.json"

echo "ci-smoke-aws ok"
