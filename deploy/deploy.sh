#!/usr/bin/env bash
# Builds Game of Drones and puts it online at https://primo-ideas.xyz/drones/:
# the world server for Linux (static musl binary, linked with rust-lld) and
# the browser client, then upload and install. The machine must have been
# prepared once with setup-drones.sh:
#
#   scp deploy/setup-drones.sh root@primo-ideas.xyz:/tmp/ && ssh root@primo-ideas.xyz sh /tmp/setup-drones.sh
#
#   ./deploy/deploy.sh                 the usual run
#   SERVER=root@other.host ./deploy/deploy.sh
set -euo pipefail
cd "$(dirname "$0")/.."

SERVER=${SERVER:-root@primo-ideas.xyz}
URL=${URL:-https://primo-ideas.xyz/drones/}
stage=target/deploy
archive=target/drones-deploy.tar.gz

echo "==> serveur de monde (Linux)"
cargo build --locked --release --target x86_64-unknown-linux-musl --bin god-server

echo "==> client web"
./deploy/build-web.sh >/dev/null

echo "==> préparation"
rm -rf "$stage"
mkdir -p "$stage"
tr -d '\r' < deploy/install.sh > "$stage/install.sh"
cp target/x86_64-unknown-linux-musl/release/god-server "$stage/"
cp -r dist/drones "$stage/web"
tar -czf "$archive" -C "$stage" .

echo "==> envoi vers $SERVER"
scp -o BatchMode=yes "$archive" "$SERVER:/tmp/drones-deploy.tar.gz"
# Plain newlines whatever this file has: a stray \r would break the remote shell.
ssh -o BatchMode=yes "$SERVER" "$(tr -d '\r' <<'EOF'
set -e
rm -rf /tmp/drones-deploy
mkdir -p /tmp/drones-deploy
tar -xzf /tmp/drones-deploy.tar.gz -C /tmp/drones-deploy
cd /tmp/drones-deploy
sh install.sh
rm -rf /tmp/drones-deploy /tmp/drones-deploy.tar.gz
EOF
)"

echo "==> vérification de $URL"
curl -fsS -o /dev/null -w "    page %{http_code}\n" "$URL"
curl -fsS -o /dev/null -w "    module %{http_code}, %{content_type}\n" "${URL}game_of_drones.wasm"
echo "Game of Drones est en ligne : $URL"
