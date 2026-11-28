#!/bin/bash
# Starts the relay under wrangler dev with TLS (a self-signed certificate) in the background and waits until it
# answers. The native transport tests run against it.
#   dev-tls.sh [port] [bind address]
set -euo pipefail

port="${1:-8788}"
ip="${2:-127.0.0.1}"
cd "$(dirname "$0")/.."
log="${RELAY_LOG:-$PWD/wrangler-tls.log}"
npx wrangler dev --local --local-protocol https --ip "$ip" --port "$port" --inspector-port "$((port + 1000))" > "$log" 2>&1 &
for _ in $(seq 90); do
  if curl -sfk -m 5 "https://127.0.0.1:$port/relay/v1/group/$(printf 'a%.0s' $(seq 32))" > /dev/null; then
    exit 0
  fi
  sleep 1
done
cat "$log"
exit 1
