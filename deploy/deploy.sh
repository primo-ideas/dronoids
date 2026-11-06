#!/usr/bin/env bash
# Builds Dronoids and puts it online at https://primo-ideas.xyz/dronoids/:
# the world server for Linux (static musl binary, linked with rust-lld) and
# the browser client, then upload and install. The machine must have been
# prepared once with setup-dronoids.sh:
#
#   scp deploy/setup-dronoids.sh root@primo-ideas.xyz:/tmp/ && ssh root@primo-ideas.xyz sh /tmp/setup-dronoids.sh
#
#   ./deploy/deploy.sh                 the usual run
#   SERVER=root@other.host ./deploy/deploy.sh
set -euo pipefail
cd "$(dirname "$0")/.."

SERVER=${SERVER:-root@primo-ideas.xyz}
URL=${URL:-https://primo-ideas.xyz/dronoids/}
stage=target/deploy
archive=target/dronoids-deploy.tar.gz

echo "==> serveur de monde (Linux)"
cargo build --locked --release --target x86_64-unknown-linux-musl --bin dronoids-server

echo "==> client web"
./deploy/build-web.sh >/dev/null

echo "==> préparation"
rm -rf "$stage"
mkdir -p "$stage"
tr -d '\r' < deploy/install.sh > "$stage/install.sh"
cp target/x86_64-unknown-linux-musl/release/dronoids-server "$stage/"
cp -r dist/dronoids "$stage/web"
tar -czf "$archive" -C "$stage" .

echo "==> envoi vers $SERVER"
scp -o BatchMode=yes "$archive" "$SERVER:/tmp/dronoids-deploy.tar.gz"
# Plain newlines whatever this file has: a stray \r would break the remote shell.
ssh -o BatchMode=yes "$SERVER" "$(tr -d '\r' <<'EOF'
set -e
rm -rf /tmp/dronoids-deploy
mkdir -p /tmp/dronoids-deploy
tar -xzf /tmp/dronoids-deploy.tar.gz -C /tmp/dronoids-deploy
cd /tmp/dronoids-deploy
sh install.sh
rm -rf /tmp/dronoids-deploy /tmp/dronoids-deploy.tar.gz
EOF
)"

echo "==> vérification de $URL"
curl -fsS -o /dev/null -w "    page %{http_code}\n" "$URL"
curl -fsS -o /dev/null -w "    module %{http_code}, %{content_type}\n" "${URL}dronoids.wasm"
echo "Dronoids est en ligne : $URL"
