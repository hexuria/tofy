#!/usr/bin/env bash
# Local-forward a laptop port to ElastiCache Redis.
#
# This is NOT tofy apply. It is an optional helper, not a builder method,
# and apply does not invoke it.
#
# ElastiCache has no public IP. Bind::All only widens the security-group
# CIDR; it cannot make Redis reachable from the public internet.
#
# Usage:
#   scripts/redis-tunnel.sh --host <elasticache-host> [--port 6379] [--local-port 26379]
#
# Auth — one of:
#   SSH:  --ssh user@bastion
#         or TOFY_REDIS_TUNNEL_SSH=user@bastion
#         (ssh -N -L <local-port>:<host>:<port>)
#   SSM:  --ssm-target <instance-id>
#         (aws ssm start-session port forwarding to the remote host, if available)
#
# After the tunnel is up, point Redis clients at 127.0.0.1:<local-port>
# with TLS (rediss) + AUTH from TOFY_CACHE_PASSWORD.
# TOFY_CACHE_URI from apply remains the ElastiCache rediss:// host (in-VPC).
# The laptop uses 127.0.0.1, not that hostname, unless DNS/VPN already routes there.
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  scripts/redis-tunnel.sh --host <elasticache-host> [--port 6379] [--local-port 26379]
                          (--ssh user@bastion | --ssm-target instance-id)

Local-forward a laptop port to ElastiCache. This is NOT tofy apply.

ElastiCache has no public IP. Bind::All only widens the security-group CIDR;
it cannot make Redis reachable from the public internet. apply does not
invoke this script. Default TOFY_CACHE_URI on Aws stays rediss:// to the
ElastiCache host (in-VPC).

Options:
  --host <elasticache-host>   ElastiCache hostname (required)
  --port <port>               Remote Redis port (default: 6379)
  --local-port <port>         Laptop listen port (default: 26379)
  --ssh <user@bastion>        SSH jump host (or set TOFY_REDIS_TUNNEL_SSH)
  --ssm-target <instance-id>  SSM managed instance to port-forward through
  -h, --help                  Show this help

After the tunnel is up, point Redis clients at 127.0.0.1:<local-port> with
TLS (rediss) and AUTH from TOFY_CACHE_PASSWORD. Do not use the ElastiCache
hostname from TOFY_CACHE_URI unless DNS/VPN already routes there.
EOF
}

die() {
  echo "error: $*" >&2
  exit 1
}

need_value() {
  local flag="$1"
  local value="${2:-}"
  if [[ -z "$value" || "$value" == -* ]]; then
    die "$flag requires a value"
  fi
}

is_port() {
  [[ "$1" =~ ^[0-9]+$ ]] && ((10#$1 >= 1 && 10#$1 <= 65535))
}

host=""
port="6379"
local_port="26379"
ssh_target="${TOFY_REDIS_TUNNEL_SSH:-}"
ssh_from_cli=0
ssm_target=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    -h|--help)
      usage
      exit 0
      ;;
    --host)
      need_value "$1" "${2:-}"
      host="$2"
      shift 2
      ;;
    --port)
      need_value "$1" "${2:-}"
      port="$2"
      shift 2
      ;;
    --local-port)
      need_value "$1" "${2:-}"
      local_port="$2"
      shift 2
      ;;
    --ssh)
      need_value "$1" "${2:-}"
      ssh_target="$2"
      ssh_from_cli=1
      shift 2
      ;;
    --ssm-target)
      need_value "$1" "${2:-}"
      ssm_target="$2"
      shift 2
      ;;
    *)
      die "unknown argument: $1 (see --help)"
      ;;
  esac
done

[[ -n "$host" ]] || die "--host <elasticache-host> is required (see --help)"
[[ "$host" =~ ^[A-Za-z0-9._-]+$ ]] || die "--host must be a hostname or IPv4 address"
is_port "$port" || die "--port must be an integer 1–65535"
is_port "$local_port" || die "--local-port must be an integer 1–65535"

if [[ "$ssh_from_cli" -eq 1 && -n "$ssm_target" ]]; then
  die "provide either --ssh or --ssm-target, not both"
fi

if [[ -n "$ssm_target" ]]; then
  mode="ssm"
elif [[ -n "$ssh_target" ]]; then
  mode="ssh"
else
  die "neither SSH nor SSM target provided. Pass --ssh user@bastion (or TOFY_REDIS_TUNNEL_SSH) or --ssm-target instance-id. ElastiCache has no public IP, so a tunnel or VPN is required."
fi

cat <<EOF
================================================================
scripts/redis-tunnel.sh is NOT tofy apply.

ElastiCache has no public IP. Bind::All only widens the security-group
CIDR; it cannot make Redis reachable from the public internet.

Point Redis clients at 127.0.0.1:${local_port} with TLS (rediss) and AUTH
from TOFY_CACHE_PASSWORD. TOFY_CACHE_URI from apply remains the in-VPC
ElastiCache rediss:// host; the laptop uses 127.0.0.1, not that hostname,
unless DNS/VPN already routes there.
================================================================
EOF

if [[ "$mode" == "ssh" ]]; then
  if ! command -v ssh >/dev/null 2>&1; then
    die "ssh is required for --ssh / TOFY_REDIS_TUNNEL_SSH"
  fi
  echo "Forwarding 127.0.0.1:${local_port} -> ${host}:${port} via ssh ${ssh_target} (ssh -N -L)."
  echo "Stop with Ctrl-C."
  exec ssh -N -L "${local_port}:${host}:${port}" "$ssh_target"
fi

if ! command -v aws >/dev/null 2>&1; then
  die "aws CLI is required for --ssm-target"
fi
if ! command -v session-manager-plugin >/dev/null 2>&1; then
  die "SSM port forwarding is not available: install the Session Manager plugin (session-manager-plugin) for the AWS CLI"
fi

params=$(printf '{"host":["%s"],"portNumber":["%s"],"localPortNumber":["%s"]}' "$host" "$port" "$local_port")
echo "Forwarding 127.0.0.1:${local_port} -> ${host}:${port} via SSM target ${ssm_target}."
echo "Stop with Ctrl-C."
exec aws ssm start-session \
  --target "$ssm_target" \
  --document-name AWS-StartPortForwardingSessionToRemoteHost \
  --parameters "$params"
