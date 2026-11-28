#!/bin/bash
# Starts the relay under wrangler dev with TLS in the background and waits until it answers. The certificate comes from
# a certificate authority made here (relay/.dev-tls/ca.pem), which the test machine has to trust: the relay client
# judges certificates through the system's trust store, so the scripts that run the native tests install it first
# (certutil on Windows, security on macOS, the network security config of the test app on Android).
#   dev-tls.sh [port] [bind address]
set -euo pipefail

port="${1:-8788}"
ip="${2:-127.0.0.1}"
cd "$(dirname "$0")/.."
certs="$PWD/.dev-tls"
log="${RELAY_LOG:-$PWD/wrangler-tls.log}"

if [ ! -f "$certs/server.pem" ]; then
  mkdir -p "$certs"
  export MSYS2_ARG_CONV_EXCL="/CN="
  openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 30 -subj "/CN=Lazy relay dev CA" \
    -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign,cRLSign" \
    -keyout "$certs/ca.key" -out "$certs/ca.pem"
  openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "/CN=localhost" \
    -keyout "$certs/server.key" -out "$certs/server.csr"
  # 10.0.2.2 is how an Android emulator reaches the host
  printf 'subjectAltName=DNS:localhost,IP:127.0.0.1,IP:10.0.2.2\nbasicConstraints=CA:FALSE\nkeyUsage=digitalSignature\nextendedKeyUsage=serverAuth\n' > "$certs/server.ext"
  openssl x509 -req -in "$certs/server.csr" -CA "$certs/ca.pem" -CAkey "$certs/ca.key" -CAcreateserial -days 30 \
    -extfile "$certs/server.ext" -out "$certs/server.pem"
fi

npx wrangler dev --local --local-protocol https --https-key-path "$certs/server.key" --https-cert-path "$certs/server.pem" \
  --ip "$ip" --port "$port" --inspector-port "$((port + 1000))" > "$log" 2>&1 &
for _ in $(seq 90); do
  if curl -sfk -m 5 "https://127.0.0.1:$port/relay/v1/group/$(printf 'a%.0s' $(seq 32))" > /dev/null; then
    exit 0
  fi
  sleep 1
done
cat "$log"
exit 1
