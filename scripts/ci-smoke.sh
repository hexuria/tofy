#!/usr/bin/env bash
# Real provision smoke: apply the public infra crate, probe published host ports, destroy.
# Must fail if Docker is missing. Do not skip.
set -euo pipefail

ROOT="${TOFY_SMOKE_DIR:-examples/infra}"
BIN=(cargo run -q -p tofy -- --dir "$ROOT")
export TOFY_SMOKE_ROOT="$ROOT"

if ! docker info >/dev/null 2>&1; then
  echo "Docker is required for this smoke; refusing to treat emit-only as success."
  exit 1
fi

echo "== apply (public path: cargo run -p infra) =="
set +e
cargo run -p infra -- --dir "$ROOT" apply | tee /tmp/tofy-apply.log
APPLY_EC=${PIPESTATUS[0]}
set -e
if grep -q "Docker is not available" /tmp/tofy-apply.log; then
  echo "apply claimed Docker is missing; CI must fail"
  exit 1
fi
if [[ "$APPLY_EC" -ne 0 ]]; then
  echo "apply exited $APPLY_EC"
  exit "$APPLY_EC"
fi
if ! grep -q "Applied." /tmp/tofy-apply.log; then
  echo "apply did not print Applied."
  exit 1
fi

echo "== state is Applied =="
python3 - <<PY
import json, sys
from pathlib import Path
p = Path("$ROOT") / ".tofy" / "state.json"
state = json.loads(p.read_text())
resources = state.get("resources") or {}
if not resources:
    sys.exit("state.json has no resources")
for name, r in resources.items():
    status = r.get("status")
    if status != "applied":
        sys.exit(f"{name} status={status!r}, expected applied (not emitted)")
print("all resources status=applied:", ", ".join(sorted(resources)))
PY

echo "== load published ports from outputs =="
if [[ ! -f "$ROOT/.tofy/outputs.json" && ! -f "$ROOT/.tofy/outputs.env" ]]; then
  echo "apply did not write .tofy/outputs.json or outputs.env"
  exit 1
fi
eval "$(python3 - <<'PY'
import json, os, pathlib, shlex, sys

root = pathlib.Path(os.environ["TOFY_SMOKE_ROOT"]) / ".tofy"
js, envp = root / "outputs.json", root / "outputs.env"
data = {}
if js.exists():
    data = json.loads(js.read_text())
elif envp.exists():
    for line in envp.read_text().splitlines():
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        data[key] = value
need = [
    "TOFY_APPDB_PORT",
    "TOFY_CACHE_PORT",
    "TOFY_CACHE_PASSWORD",
    "TOFY_UPLOADS_PORT",
    "TOFY_UPLOADS_BUCKET",
    "TOFY_UPLOADS_ENDPOINT",
    "TOFY_UPLOADS_ACCESS_KEY",
    "TOFY_UPLOADS_SECRET_KEY",
    "TOFY_NETWORK",
    "TOFY_APPDB_URI",
]
missing = [k for k in need if not data.get(k)]
if missing:
    sys.exit("outputs missing keys: " + ", ".join(missing))
for key in need:
    print(f"{key}={shlex.quote(str(data[key]))}")
PY
)"
export TOFY_APPDB_PORT TOFY_CACHE_PORT TOFY_CACHE_PASSWORD TOFY_UPLOADS_PORT \
  TOFY_UPLOADS_BUCKET TOFY_UPLOADS_ENDPOINT TOFY_UPLOADS_ACCESS_KEY TOFY_UPLOADS_SECRET_KEY \
  TOFY_NETWORK TOFY_APPDB_URI
echo "TOFY_APPDB_PORT=$TOFY_APPDB_PORT TOFY_CACHE_PORT=$TOFY_CACHE_PORT TOFY_UPLOADS_PORT=$TOFY_UPLOADS_PORT TOFY_UPLOADS_BUCKET=$TOFY_UPLOADS_BUCKET TOFY_NETWORK=$TOFY_NETWORK"

echo "== containers are running =="
running_names="$(docker ps --format '{{.Names}}')"
for name in tofy-demo-appdb tofy-demo-cache tofy-demo-uploads; do
  if ! grep -qx "$name" <<<"$running_names"; then
    echo "container $name is not running"
    docker ps -a
    exit 1
  fi
  echo "running $name"
done

pg_isready_host() {
  # Probe the published host port. Never the default unix socket.
  if command -v pg_isready >/dev/null 2>&1; then
    pg_isready -h 127.0.0.1 -p "$TOFY_APPDB_PORT"
  else
    docker run --rm --network host postgres:16 \
      pg_isready -h 127.0.0.1 -p "$TOFY_APPDB_PORT"
  fi
}

echo "== postgres accepts connections on published host port =="
ready=0
deadline=$((SECONDS + 30))
while (( SECONDS < deadline )); do
  if pg_isready_host; then
    ready=1
    break
  fi
  sleep 0.5
done
if [[ "$ready" -ne 1 ]]; then
  echo "pg_isready -h 127.0.0.1 -p $TOFY_APPDB_PORT failed for ~30s"
  exit 1
fi

echo "== redis on published host port =="
if command -v redis-cli >/dev/null 2>&1; then
  pong="$(REDISCLI_AUTH="$TOFY_CACHE_PASSWORD" redis-cli -h 127.0.0.1 -p "$TOFY_CACHE_PORT" ping)"
  if [[ "$pong" != "PONG" ]]; then
    echo "redis-cli AUTH ping => $pong"
    exit 1
  fi
  echo "redis-cli AUTH PONG on 127.0.0.1:$TOFY_CACHE_PORT"
fi
python3 - <<PY
import socket, sys, time

def wait_tcp(port, seconds=30):
    deadline = time.time() + seconds
    last = None
    while time.time() < deadline:
        s = socket.socket()
        s.settimeout(1)
        try:
            s.connect(("127.0.0.1", port))
            s.close()
            return
        except OSError as e:
            last = e
            time.sleep(0.3)
    sys.exit(f"127.0.0.1:{port} did not accept TCP ({last})")

for label, port in (
    ("postgres", int("$TOFY_APPDB_PORT")),
    ("redis", int("$TOFY_CACHE_PORT")),
    ("uploads", int("$TOFY_UPLOADS_PORT")),
):
    wait_tcp(port)
    print(f"tcp 127.0.0.1:{port} ({label}) ok")
PY

echo "== object-store bucket exists =="
python3 - <<'PY'
import datetime, hashlib, hmac, http.client, os, sys
from urllib.parse import urlparse

bucket = os.environ["TOFY_UPLOADS_BUCKET"]
endpoint = os.environ["TOFY_UPLOADS_ENDPOINT"]
access = os.environ["TOFY_UPLOADS_ACCESS_KEY"]
secret = os.environ["TOFY_UPLOADS_SECRET_KEY"]
if bucket != "uploads":
    sys.exit(f"TOFY_UPLOADS_BUCKET={bucket!r}, expected 'uploads'")

parsed = urlparse(endpoint)
host = parsed.netloc
path = f"/{bucket}"
now = datetime.datetime.utcnow()
amz_date = now.strftime("%Y%m%dT%H%M%SZ")
datestamp = now.strftime("%Y%m%d")
region = "us-east-1"
payload = hashlib.sha256(b"").hexdigest()
canonical_headers = f"host:{host}\nx-amz-content-sha256:{payload}\nx-amz-date:{amz_date}\n"
signed_headers = "host;x-amz-content-sha256;x-amz-date"
canonical = f"HEAD\n{path}\n\n{canonical_headers}\n{signed_headers}\n{payload}"
scope = f"{datestamp}/{region}/s3/aws4_request"
string_to_sign = f"AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{hashlib.sha256(canonical.encode()).hexdigest()}"

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
conn = http.client.HTTPConnection(host, timeout=5)
conn.request(
    "HEAD",
    path,
    headers={
        "Host": host,
        "x-amz-date": amz_date,
        "x-amz-content-sha256": payload,
        "Authorization": auth,
    },
)
resp = conn.getresponse()
resp.read()
if resp.status not in (200, 204):
    sys.exit(f"HEAD {endpoint}{path} -> {resp.status} (bucket missing?)")
print(f"bucket {bucket} exists at {endpoint}")
PY

echo "== tofy run injects TOFY_APPDB_URI =="
"${BIN[@]}" run -- python3 -c '
import os, sys
u = os.environ.get("TOFY_APPDB_URI", "")
if not u.startswith("postgres://"):
    sys.exit("TOFY_APPDB_URI missing or not a postgres uri")
if "@127.0.0.1:" not in u:
    sys.exit("TOFY_APPDB_URI is not the host loopback uri")
port = os.environ.get("TOFY_APPDB_PORT", "")
if port and f"@127.0.0.1:{port}/" not in u:
    sys.exit(f"TOFY_APPDB_URI port does not match TOFY_APPDB_PORT={port}")
print("TOFY_APPDB_URI is set for the host")
'

echo "== destroy =="
"${BIN[@]}" destroy

echo "== containers and stack network are gone =="
leftover_names="$(docker ps -a --format '{{.Names}}')"
for name in tofy-demo-appdb tofy-demo-cache tofy-demo-uploads; do
  if grep -qx "$name" <<<"$leftover_names"; then
    echo "container $name still exists after destroy"
    docker ps -a
    exit 1
  fi
done
if docker network inspect "$TOFY_NETWORK" >/dev/null 2>&1; then
  echo "stack network $TOFY_NETWORK still exists after destroy"
  docker network ls
  exit 1
fi
echo "destroy removed stack containers and $TOFY_NETWORK"

if [[ -f "$ROOT/.tofy/outputs.env" ]]; then
  echo "outputs.env still present after destroy"
  exit 1
fi

echo "ci-smoke ok"
