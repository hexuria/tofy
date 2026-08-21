#!/usr/bin/env bash
# Real provision smoke: apply the public mysql infra crate, probe published host port, destroy.
# Must fail if Docker is missing. Do not skip.
# MYSQL ONLY — no redis/minio/drift-stop-cache.
set -euo pipefail

ROOT="${TOFY_SMOKE_DIR:-examples/infra-mysql}"
BIN=(cargo run -q -p tofy -- --dir "$ROOT")
CONTAINER=tofy-demomysql-appmysql
export TOFY_SMOKE_ROOT="$ROOT"

if ! docker info >/dev/null 2>&1; then
  echo "Docker is required for this smoke; refusing to treat emit-only as success."
  exit 1
fi

echo "== apply (public path: cargo run -p infra-mysql) =="
set +e
cargo run -p infra-mysql -- --dir "$ROOT" apply | tee /tmp/tofy-mysql-apply.log
APPLY_EC=${PIPESTATUS[0]}
set -e
if grep -q "Docker is not available" /tmp/tofy-mysql-apply.log; then
  echo "apply claimed Docker is missing; CI must fail"
  exit 1
fi
if [[ "$APPLY_EC" -ne 0 ]]; then
  echo "apply exited $APPLY_EC"
  exit "$APPLY_EC"
fi
if ! grep -q "Applied." /tmp/tofy-mysql-apply.log; then
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
    "TOFY_APPMYSQL_URI",
    "TOFY_APPMYSQL_PORT",
    "TOFY_NETWORK",
]
missing = [k for k in need if not data.get(k)]
if missing:
    sys.exit("outputs missing keys: " + ", ".join(missing))
for key in need:
    print(f"{key}={shlex.quote(str(data[key]))}")
if data.get("TOFY_APPMYSQL_PASSWORD"):
    print(f"TOFY_APPMYSQL_PASSWORD={shlex.quote(str(data['TOFY_APPMYSQL_PASSWORD']))}")
PY
)"
export TOFY_APPMYSQL_URI TOFY_APPMYSQL_PORT TOFY_NETWORK
if [[ -n "${TOFY_APPMYSQL_PASSWORD:-}" ]]; then
  export TOFY_APPMYSQL_PASSWORD
fi
echo "TOFY_APPMYSQL_PORT=$TOFY_APPMYSQL_PORT TOFY_NETWORK=$TOFY_NETWORK"

echo "== containers are running =="
running_names="$(docker ps --format '{{.Names}}')"
if ! grep -qx "$CONTAINER" <<<"$running_names"; then
  echo "container $CONTAINER is not running"
  docker ps -a
  exit 1
fi
echo "running $CONTAINER"

mysqladmin_host() {
  # Probe the published host port. Never the default unix socket.
  local extra=()
  if [[ -n "${TOFY_APPMYSQL_PASSWORD:-}" ]]; then
    extra=(-uroot "-p${TOFY_APPMYSQL_PASSWORD}")
  fi
  if command -v mysqladmin >/dev/null 2>&1; then
    mysqladmin ping -h 127.0.0.1 -P "$TOFY_APPMYSQL_PORT" --silent "${extra[@]}"
  else
    docker run --rm --network host mysql:8 \
      mysqladmin ping -h 127.0.0.1 -P "$TOFY_APPMYSQL_PORT" --silent "${extra[@]}"
  fi
}

echo "== mysql accepts connections on published host port =="
ready=0
deadline=$((SECONDS + 60))
while (( SECONDS < deadline )); do
  if mysqladmin_host; then
    ready=1
    break
  fi
  sleep 0.5
done
if [[ "$ready" -ne 1 ]]; then
  echo "mysqladmin ping -h 127.0.0.1 -P $TOFY_APPMYSQL_PORT failed for ~60s"
  exit 1
fi

echo "== tcp on published host port =="
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

port = int("$TOFY_APPMYSQL_PORT")
wait_tcp(port)
print(f"tcp 127.0.0.1:{port} (mysql) ok")
PY

echo "== tofy run injects TOFY_APPMYSQL_URI =="
"${BIN[@]}" run -- python3 -c '
import os, sys
u = os.environ.get("TOFY_APPMYSQL_URI", "")
if not u.startswith("mysql://"):
    sys.exit("TOFY_APPMYSQL_URI missing or not a mysql uri")
if "@127.0.0.1:" not in u:
    sys.exit("TOFY_APPMYSQL_URI is not the host loopback uri")
port = os.environ.get("TOFY_APPMYSQL_PORT", "")
if port and f"@127.0.0.1:{port}/" not in u:
    sys.exit(f"TOFY_APPMYSQL_URI port does not match TOFY_APPMYSQL_PORT={port}")
print("TOFY_APPMYSQL_URI is set for the host")
'

echo "== drift: stop mysql, plan must show a change =="
if ! docker stop "$CONTAINER" >/dev/null; then
  echo "failed to stop $CONTAINER"
  exit 1
fi
set +e
"${BIN[@]}" plan | tee /tmp/tofy-mysql-plan-drift.log
PLAN_EC=${PIPESTATUS[0]}
set -e
if [[ "$PLAN_EC" -ne 0 ]]; then
  echo "plan exited $PLAN_EC"
  exit "$PLAN_EC"
fi
if grep -q "No changes." /tmp/tofy-mysql-plan-drift.log; then
  echo "plan ignored a stopped container"
  exit 1
fi

echo "== apply heals drift =="
set +e
"${BIN[@]}" apply | tee /tmp/tofy-mysql-heal.log
HEAL_EC=${PIPESTATUS[0]}
set -e
if [[ "$HEAL_EC" -ne 0 ]]; then
  echo "heal apply exited $HEAL_EC"
  exit "$HEAL_EC"
fi
if ! grep -q "Applied." /tmp/tofy-mysql-heal.log; then
  echo "heal apply did not print Applied."
  exit 1
fi
if ! docker ps --format '{{.Names}}' | grep -qx "$CONTAINER"; then
  echo "$CONTAINER is not running after heal"
  docker ps -a
  exit 1
fi

echo "== destroy =="
"${BIN[@]}" destroy

echo "== containers and stack network are gone =="
leftover_names="$(docker ps -a --format '{{.Names}}')"
if grep -qx "$CONTAINER" <<<"$leftover_names"; then
  echo "container $CONTAINER still exists after destroy"
  docker ps -a
  exit 1
fi
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

echo "ci-smoke-mysql ok"
