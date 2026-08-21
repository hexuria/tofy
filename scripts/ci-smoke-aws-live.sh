#!/usr/bin/env bash
# Optional live apply of Backend::Aws. Unique stack name per run.
# Missing tofu fails. Missing AWS credentials skip-as-success only when
# TOFY_AWS_LIVE_OPTIONAL=1 (the opt-in workflow sets that). Required CI is
# scripts/ci-smoke-aws.sh (emit + tofu validate) and must stay validate-only.
# User-facing commands stay `tofy apply` / `tofy plan` / `tofy destroy`.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO"

EXAMPLE_DIR="examples/infra-aws"
EXAMPLE_TOFY="$REPO/$EXAMPLE_DIR/.tofy"
WORKDIR=""
CLEANED=0
DESTROY_LOG=/tmp/tofy-aws-live-destroy.log
APPLY_LOG=/tmp/tofy-aws-live-apply.log

# Never open the world. Do not export TOFY_APPLIER_CIDR=0.0.0.0/0; let
# `tofy apply` discover the runner public IP (or honor a real /32).
if [[ "${TOFY_APPLIER_CIDR:-}" == "0.0.0.0/0" || "${TOFY_APPLIER_CIDR:-}" == "0.0.0.0" ]]; then
  echo "TOFY_APPLIER_CIDR=${TOFY_APPLIER_CIDR} is not allowed; unsetting so tofy apply can discover a /32"
  unset TOFY_APPLIER_CIDR
fi

if ! command -v tofu >/dev/null 2>&1; then
  echo "OpenTofu engine is required for this backend; refusing to treat emit-only as success."
  exit 1
fi
tofu version

aws_creds_available() {
  if [[ -n "${AWS_ACCESS_KEY_ID:-}" && -n "${AWS_SECRET_ACCESS_KEY:-}" ]]; then
    return 0
  fi
  if [[ -n "${AWS_CONTAINER_CREDENTIALS_RELATIVE_URI:-}" || -n "${AWS_CONTAINER_CREDENTIALS_FULL_URI:-}" ]]; then
    return 0
  fi
  if [[ -n "${AWS_WEB_IDENTITY_TOKEN_FILE:-}" ]]; then
    return 0
  fi
  local creds="${AWS_SHARED_CREDENTIALS_FILE:-$HOME/.aws/credentials}"
  local cfg="${AWS_CONFIG_FILE:-$HOME/.aws/config}"
  [[ -s "$creds" || -s "$cfg" ]]
}

if [[ -z "${AWS_SESSION_TOKEN:-}" ]]; then
  unset AWS_SESSION_TOKEN
fi

if ! aws_creds_available; then
  if [[ "${TOFY_AWS_LIVE_OPTIONAL:-}" == "1" ]]; then
    echo "AWS credentials were not found; skipping optional live apply."
    exit 0
  fi
  echo "AWS credentials were not found in the environment; did not apply."
  exit 1
fi

SUFFIX="${TOFY_AWS_LIVE_SUFFIX:-${GITHUB_RUN_ID:-local}}"
SUFFIX="$(printf '%s' "$SUFFIX" | tr -c 'A-Za-z0-9_-' '-' | sed 's/^-*//;s/-*$//')"
if [[ -z "$SUFFIX" ]]; then
  SUFFIX=local
fi
PROJECT="demoaws-${SUFFIX}"
echo "live stack project=$PROJECT"

WORKDIR="$(mktemp -d "${TMPDIR:-/tmp}/tofy-aws-live.XXXXXX")"
SPEC="$WORKDIR/spec.json"
echo "WORKDIR=$WORKDIR"

BIN=(cargo run -q -p tofy -- --dir "$WORKDIR" --spec "$SPEC")

scrub_example_tofy() {
  if [[ -d "$EXAMPLE_TOFY" ]]; then
    rm -rf "$EXAMPLE_TOFY"
  fi
}

cleanup() {
  # Idempotent: EXIT and ERR both fire on `set -e` failures. Destroy errors
  # are printed here; the main path asserts Destroyed after a successful apply.
  if [[ "$CLEANED" -eq 1 ]]; then
    return 0
  fi
  CLEANED=1
  set +e
  scrub_example_tofy
  if [[ -n "$WORKDIR" && -d "$WORKDIR" ]]; then
    echo "== tofy destroy =="
    "${BIN[@]}" destroy 2>&1 | tee "$DESTROY_LOG"
    destroy_ec=${PIPESTATUS[0]}
    if [[ "$destroy_ec" -ne 0 ]]; then
      echo "tofy destroy exited $destroy_ec (printed above; destroy still ran after apply)"
    fi
  fi
  set -e
  return 0
}

trap cleanup EXIT
trap cleanup ERR

echo "== emit (public path: cargo run -p infra-aws -- --dir $EXAMPLE_DIR emit) =="
cargo run -p infra-aws -- --dir "$EXAMPLE_DIR" emit
if [[ ! -f "$EXAMPLE_TOFY/spec.json" ]]; then
  echo "emit did not write $EXAMPLE_DIR/.tofy/spec.json"
  exit 1
fi
cp "$EXAMPLE_TOFY/spec.json" "$SPEC"
# Do not keep emit artifacts (or live state) under the example tree.
scrub_example_tofy

python3 - "$SPEC" "$PROJECT" <<'PY'
import json, sys

path, project = sys.argv[1], sys.argv[2]
with open(path, encoding="utf-8") as f:
    spec = json.load(f)
spec["project"] = project
if spec.get("backend") != "aws":
    sys.exit(f"spec backend={spec.get('backend')!r}, expected aws")
names = [r.get("name") for r in spec.get("resources") or []]
for need in ("appdb", "cache", "uploads"):
    if need not in names:
        sys.exit(f"spec missing resource {need}, have {names}")
with open(path, "w", encoding="utf-8") as f:
    json.dump(spec, f, indent=2)
    f.write("\n")
print(f"rewrote spec.project to {project}; resources stay {', '.join(names)}")
PY

echo "== tofy apply =="
set +e
"${BIN[@]}" apply 2>&1 | tee "$APPLY_LOG"
APPLY_EC=${PIPESTATUS[0]}
set -e
if grep -qi "go run tofu" "$APPLY_LOG"; then
  echo "apply told the user to run tofu"
  exit 1
fi
if [[ "$APPLY_EC" -ne 0 ]]; then
  echo "tofy apply exited $APPLY_EC"
  exit "$APPLY_EC"
fi
if ! grep -q "Applied." "$APPLY_LOG"; then
  echo "tofy apply did not print Applied."
  exit 1
fi

echo "== load outputs (RDS / S3; do not print secrets) =="
if [[ ! -f "$WORKDIR/.tofy/outputs.json" && ! -f "$WORKDIR/.tofy/outputs.env" ]]; then
  echo "apply did not write .tofy/outputs.json or outputs.env"
  exit 1
fi
export TOFY_LIVE_ROOT="$WORKDIR"
eval "$(python3 - <<'PY'
import json, os, pathlib, shlex, sys

root = pathlib.Path(os.environ["TOFY_LIVE_ROOT"]) / ".tofy"
data = {}
js, envp = root / "outputs.json", root / "outputs.env"
if js.exists():
    data = json.loads(js.read_text())
elif envp.exists():
    for line in envp.read_text().splitlines():
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        data[key] = value
need = [
    "TOFY_APPDB_URI",
    "TOFY_APPDB_HOST",
    "TOFY_APPDB_PORT",
    "TOFY_UPLOADS_BUCKET",
    "TOFY_UPLOADS_REGION",
    "TOFY_UPLOADS_ENDPOINT",
]
missing = [k for k in need if not data.get(k)]
if missing:
    sys.exit("outputs missing keys: " + ", ".join(missing))
for key in need:
    print(f"{key}={shlex.quote(str(data[key]))}")
PY
)"
export TOFY_APPDB_URI TOFY_APPDB_HOST TOFY_APPDB_PORT \
  TOFY_UPLOADS_BUCKET TOFY_UPLOADS_REGION TOFY_UPLOADS_ENDPOINT
echo "TOFY_APPDB_HOST=$TOFY_APPDB_HOST TOFY_APPDB_PORT=$TOFY_APPDB_PORT TOFY_UPLOADS_BUCKET=$TOFY_UPLOADS_BUCKET TOFY_UPLOADS_REGION=$TOFY_UPLOADS_REGION"

echo "== TOFY_APPDB_URI is postgres:// at RDS (not loopback) =="
python3 - <<'PY'
import os, sys
from urllib.parse import urlparse

uri = os.environ.get("TOFY_APPDB_URI", "")
if not uri.startswith("postgres://"):
    sys.exit("TOFY_APPDB_URI is missing or not a postgres:// URI")
host = os.environ.get("TOFY_APPDB_HOST", "")
parsed = urlparse(uri)
if parsed.hostname in ("127.0.0.1", "localhost", "::1") or host in ("127.0.0.1", "localhost", "::1"):
    sys.exit("TOFY_APPDB_URI/HOST is loopback; expected an RDS endpoint")
if "127.0.0.1" in uri:
    sys.exit("TOFY_APPDB_URI contains 127.0.0.1; expected an RDS endpoint")
print(f"TOFY_APPDB_URI is postgres:// at {parsed.hostname}:{parsed.port}")
PY

pg_isready_rds() {
  local host="$TOFY_APPDB_HOST" port="$TOFY_APPDB_PORT"
  if command -v pg_isready >/dev/null 2>&1; then
    pg_isready -h "$host" -p "$port"
  elif command -v docker >/dev/null 2>&1; then
    docker run --rm postgres:16 pg_isready -h "$host" -p "$port"
  else
    echo "pg_isready and docker are missing; cannot probe postgres"
    return 1
  fi
}

echo "== postgres accepts connections on the RDS endpoint (wait up to 12m) =="
ready=0
deadline=$((SECONDS + 720))
while (( SECONDS < deadline )); do
  if pg_isready_rds; then
    ready=1
    break
  fi
  sleep 5
done
if [[ "$ready" -ne 1 ]]; then
  echo "pg_isready -h $TOFY_APPDB_HOST -p $TOFY_APPDB_PORT failed for ~12m"
  exit 1
fi

echo "== S3 bucket exists =="
s3_ok=0
if command -v aws >/dev/null 2>&1; then
  if aws s3api head-bucket --bucket "$TOFY_UPLOADS_BUCKET" --region "$TOFY_UPLOADS_REGION"; then
    echo "aws s3api head-bucket $TOFY_UPLOADS_BUCKET ok"
    s3_ok=1
  fi
fi
if [[ "$s3_ok" -ne 1 ]]; then
  python3 - <<'PY'
import datetime, hashlib, hmac, os, ssl, sys, urllib.error, urllib.request

bucket = os.environ["TOFY_UPLOADS_BUCKET"]
region = os.environ["TOFY_UPLOADS_REGION"]
access = os.environ.get("AWS_ACCESS_KEY_ID") or ""
secret = os.environ.get("AWS_SECRET_ACCESS_KEY") or ""
token = os.environ.get("AWS_SESSION_TOKEN") or ""
if not access or not secret:
    sys.exit("AWS credentials missing for signed S3 HEAD")

host = f"{bucket}.s3.{region}.amazonaws.com"
now = datetime.datetime.utcnow()
amz_date = now.strftime("%Y%m%dT%H%M%SZ")
datestamp = now.strftime("%Y%m%d")
payload = hashlib.sha256(b"").hexdigest()
headers = [("host", host), ("x-amz-content-sha256", payload), ("x-amz-date", amz_date)]
if token:
    headers.append(("x-amz-security-token", token))
headers.sort()
canonical_headers = "".join(f"{k}:{v}\n" for k, v in headers)
signed_headers = ";".join(k for k, _ in headers)
canonical = f"HEAD\n/\n\n{canonical_headers}\n{signed_headers}\n{payload}"
scope = f"{datestamp}/{region}/s3/aws4_request"
string_to_sign = (
    f"AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n"
    + hashlib.sha256(canonical.encode()).hexdigest()
)

def sign(key, msg):
    return hmac.new(key, msg.encode() if isinstance(msg, str) else msg, hashlib.sha256).digest()

k = sign(("AWS4" + secret).encode(), datestamp)
k = sign(k, region)
k = sign(k, "s3")
k = sign(k, "aws4_request")
sig = hmac.new(k, string_to_sign.encode(), hashlib.sha256).hexdigest()
auth = (
    f"AWS4-HMAC-SHA256 Credential={access}/{scope}, "
    f"SignedHeaders={signed_headers}, Signature={sig}"
)
req = urllib.request.Request(f"https://{host}/", method="HEAD")
req.add_header("x-amz-date", amz_date)
req.add_header("x-amz-content-sha256", payload)
req.add_header("Authorization", auth)
if token:
    req.add_header("x-amz-security-token", token)
try:
    with urllib.request.urlopen(req, context=ssl.create_default_context(), timeout=30) as resp:
        status = getattr(resp, "status", 200)
except urllib.error.HTTPError as e:
    status = e.code
if status not in (200, 204, 301):
    sys.exit(f"HEAD https://{host}/ -> {status} (bucket missing?)")
print(f"bucket {bucket} exists in {region}")
PY
fi

echo "skip redis probe (ElastiCache has no public IP)"

echo "== tofy destroy after probes =="
cleanup
if [[ ! -f "$DESTROY_LOG" ]] || ! grep -q "Destroyed" "$DESTROY_LOG"; then
  echo "tofy destroy did not print Destroyed"
  exit 1
fi
if grep -qi "go run tofu" "$DESTROY_LOG"; then
  echo "destroy told the user to run tofu"
  exit 1
fi

echo "== second tofy destroy / plan must not claim Applied leftovers =="
set +e
"${BIN[@]}" destroy 2>&1 | tee /tmp/tofy-aws-live-destroy2.log
DESTROY2_EC=${PIPESTATUS[0]}
"${BIN[@]}" plan 2>&1 | tee /tmp/tofy-aws-live-plan2.log
PLAN2_EC=${PIPESTATUS[0]}
set -e
if grep -q "Applied." /tmp/tofy-aws-live-destroy2.log; then
  echo "second destroy claimed Applied leftovers"
  cat /tmp/tofy-aws-live-destroy2.log
  exit 1
fi
if grep -q "Applied." /tmp/tofy-aws-live-plan2.log; then
  echo "plan after destroy claimed Applied leftovers"
  cat /tmp/tofy-aws-live-plan2.log
  exit 1
fi
if grep -qi "go run tofu" /tmp/tofy-aws-live-destroy2.log /tmp/tofy-aws-live-plan2.log; then
  echo "second destroy/plan told the user to run tofu"
  exit 1
fi
python3 - <<PY
import json, sys
from pathlib import Path
state_path = Path("$WORKDIR") / ".tofy" / "state.json"
if state_path.exists():
    state = json.loads(state_path.read_text())
    for name, r in (state.get("resources") or {}).items():
        if r.get("status") == "applied":
            sys.exit(f"second destroy left {name} status=applied")
print("no Applied leftovers in state")
PY
if [[ -f "$WORKDIR/.tofy/outputs.env" ]]; then
  echo "outputs.env still present after destroy"
  exit 1
fi
echo "second destroy exit=$DESTROY2_EC plan exit=$PLAN2_EC (Applied leftovers absent)"

echo "ci-smoke-aws-live ok"
