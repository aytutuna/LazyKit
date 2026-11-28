#!/bin/bash
# Starts the relay under wrangler dev with TLS in the background and waits until it answers. The certificate comes from
# a certificate authority made here (relay/.dev-tls/ca.pem), which the test machine has to trust: the relay client
# judges certificates through the system's trust store, so the scripts that run the native tests install it first
# (certutil on Windows, security on macOS, the user certificate store on an Android emulator).
# The account service runs too, on the dev mail sink (scripts/dev-mail.mjs, MAIL_SINK for the tests) and a bot check
# that accepts "dev", as dev-account.sh sets it up.
#   dev-tls.sh [port] [bind address] [mail sink port]
set -euo pipefail

port="${1:-8788}"
ip="${2:-127.0.0.1}"
sink="${3:-8790}"
cd "$(dirname "$0")/.."
certs="$PWD/.dev-tls"
log="${RELAY_LOG:-$PWD/wrangler-tls.log}"

# Made again when there is none or the certificate is about to expire (they last 30 days). The authority may only
# vouch for the names the relay is reached by, so a machine that trusts it trusts it for nothing else.
if [ ! -f "$certs/server.pem" ] || ! openssl x509 -checkend 86400 -noout -in "$certs/server.pem" > /dev/null; then
  rm -rf "$certs"
  mkdir -p "$certs"
  export MSYS2_ARG_CONV_EXCL="/CN="
  openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 30 -subj "/CN=Lazy relay dev CA" \
    -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign,cRLSign" \
    -addext "nameConstraints=critical,permitted;DNS:localhost,permitted;IP:127.0.0.1/255.255.255.255,permitted;IP:10.0.2.2/255.255.255.255" \
    -keyout "$certs/ca.key" -out "$certs/ca.pem"
  openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "/CN=localhost" \
    -keyout "$certs/server.key" -out "$certs/server.csr"
  # 10.0.2.2 is how an Android emulator reaches the host
  printf 'subjectAltName=DNS:localhost,IP:127.0.0.1,IP:10.0.2.2\nbasicConstraints=CA:FALSE\nkeyUsage=digitalSignature\nextendedKeyUsage=serverAuth\n' > "$certs/server.ext"
  openssl x509 -req -in "$certs/server.csr" -CA "$certs/ca.pem" -CAkey "$certs/ca.key" -CAcreateserial -days 30 \
    -extfile "$certs/server.ext" -out "$certs/server.pem"
fi

node scripts/dev-mail.mjs "$sink" "$ip" &
npx wrangler dev --local --local-protocol https --https-key-path "$certs/server.key" --https-cert-path "$certs/server.pem" \
  --ip "$ip" --port "$port" --inspector-port "$((port + 1000))" \
  --var ACCOUNT_KEY:dev-account-key ID_KEY:dev-id-key PASSWORD_STAGES:1 "DEV_MAIL_URL:http://127.0.0.1:$sink/" DEV_TURNSTILE:1 > "$log" 2>&1 &
for _ in $(seq 90); do
  if curl -sfk -m 5 -X POST -d '{}' "https://127.0.0.1:$port/account/v1/nonce" > /dev/null; then
    exit 0
  fi
  sleep 1
done
cat "$log"
exit 1
