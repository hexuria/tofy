#!/usr/bin/env bash
# Real provision smoke: apply the public infra crate, probe, destroy.
# Must fail if Docker is missing. Do not skip.
set -euo pipefail

ROOT="${TOFY_SMOKE_DIR:-examples/infra}"
BIN=(cargo run -q -p tofy -- --dir "$ROOT")

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

echo "== containers are running =="
running_names="$(docker ps --format '{{.Names}}')"
for name in tofy-demo-appdb tofy-demo-cache; do
  if ! grep -qx "$name" <<<"$running_names"; then
    echo "container $name is not running"
    docker ps -a
    exit 1
  fi
  echo "running $name"
done

echo "== postgres accepts connections =="
docker exec tofy-demo-appdb pg_isready -U tofy
python3 - <<'PY'
import socket, time, sys
deadline = time.time() + 30
while time.time() < deadline:
    s = socket.socket()
    s.settimeout(1)
    try:
        s.connect(("127.0.0.1", 5433))
        s.close()
        print("tcp 127.0.0.1:5433 ok")
        sys.exit(0)
    except OSError:
        time.sleep(0.3)
sys.exit("Postgres did not accept TCP on 127.0.0.1:5433")
PY

echo "== redis PING =="
pong="$(docker exec tofy-demo-cache redis-cli PING)"
if [[ "$pong" != "PONG" ]]; then
  echo "redis-cli PING => $pong"
  exit 1
fi
python3 - <<'PY'
import socket, sys
s = socket.socket()
s.settimeout(2)
s.connect(("127.0.0.1", 6379))
s.close()
print("tcp 127.0.0.1:6379 ok")
PY

echo "== tofy run injects TOFY_APPDB_URI =="
"${BIN[@]}" run -- python3 -c '
import os, sys
u = os.environ.get("TOFY_APPDB_URI", "")
if not u.startswith("postgres://"):
    sys.exit("TOFY_APPDB_URI missing or not a postgres uri")
if "@127.0.0.1:5433/" not in u:
    sys.exit("TOFY_APPDB_URI is not the host loopback uri")
print("TOFY_APPDB_URI is set for the host")
'

echo "== destroy =="
"${BIN[@]}" destroy

echo "== containers are gone =="
leftover_names="$(docker ps -a --format '{{.Names}}')"
for name in tofy-demo-appdb tofy-demo-cache tofy-demo-uploads; do
  if grep -qx "$name" <<<"$leftover_names"; then
    echo "container $name still exists after destroy"
    docker ps -a
    exit 1
  fi
done
echo "destroy removed stack containers"

if [[ -f "$ROOT/.tofy/outputs.env" ]]; then
  echo "outputs.env still present after destroy"
  exit 1
fi

echo "ci-smoke ok"
