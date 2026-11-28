#!/bin/bash
# Starts the relay under wrangler dev with the account service working on this machine: mail goes to the sink in
# dev-mail.mjs and the bot check accepts the token "dev". Plain http, for the Rust client tests; dev-tls.sh does the
# same with TLS. Tests ask the sink at MAIL_SINK (http://127.0.0.1:8790) for the codes.
#   dev-account.sh [port] [mail sink port]
set -euo pipefail

port="${1:-8787}"
sink="${2:-8790}"
cd "$(dirname "$0")/.."
node scripts/dev-mail.mjs "$sink" 0.0.0.0 &
npx wrangler dev --local --ip 127.0.0.1 --port "$port" \
  --var ACCOUNT_KEY:dev-account-key ID_KEY:dev-id-key PASSWORD_STAGES:1 "DEV_MAIL_URL:http://127.0.0.1:$sink/" DEV_TURNSTILE:1 \
  > "${RELAY_LOG:-$PWD/wrangler.log}" 2>&1 &
for _ in $(seq 90); do
  if curl -sf -m 5 -X POST -d '{}' "http://127.0.0.1:$port/account/v1/nonce" > /dev/null; then
    exit 0
  fi
  sleep 1
done
cat "${RELAY_LOG:-$PWD/wrangler.log}"
exit 1
