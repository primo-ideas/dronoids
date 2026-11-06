#!/bin/sh
# One-time (idempotent) setup of the machine for Game of Drones, run as root.
# Creates what a deployment then only fills in:
#
#   /home/drones/                     the server's home (binary, worlds/)
#   /etc/systemd/system/drones.service its own user, loopback port 8082
#   /etc/apache2/conf-available/drones.conf   enabled with a2enconf:
#       /drones/     -> /var/www/html/drones (the web client)
#       /drones/ws   -> ws://127.0.0.1:8082/ws (the world server)
#
# It never touches the virtual hosts, TLS, the document root itself or any
# other game. Anything it overwrites is backed up first, and Apache's
# configuration is tested before it is reloaded (and put back if the test
# fails): a broken configuration would take down every site on the machine.
set -eu

PORT=8082
UNIT=/etc/systemd/system/drones.service
CONF=/etc/apache2/conf-available/drones.conf
WEB=/var/www/html/drones
HOME_DIR=/home/drones
stamp=$(date +%Y%m%d-%H%M%S)

backup() {
    if [ -e "$1" ]; then
        cp -a "$1" "$1.bak-$stamp"
        echo "  sauvegarde : $1.bak-$stamp"
    fi
}

echo "==> utilisateur drones"
if ! id drones >/dev/null 2>&1; then
    useradd --system --home-dir "$HOME_DIR" --create-home --shell /usr/sbin/nologin drones
fi
mkdir -p "$HOME_DIR/worlds" "$WEB"
chown -R drones:drones "$HOME_DIR"

echo "==> service systemd"
new_unit=$(mktemp)
cat > "$new_unit" <<EOF
# Game of Drones world server (see deploy/setup-drones.sh in the game's repository).
[Unit]
Description=Game of Drones: world server
After=network.target

[Service]
Type=simple
User=drones
Group=drones
WorkingDirectory=$HOME_DIR
ExecStart=$HOME_DIR/god-server --name primo --bind 127.0.0.1 --port $PORT --bots 6 --level normal --save $HOME_DIR/worlds/primo.world
Restart=always
RestartSec=2

NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ReadWritePaths=$HOME_DIR

[Install]
WantedBy=multi-user.target
EOF
if ! cmp -s "$new_unit" "$UNIT"; then
    backup "$UNIT"
    install -m 644 "$new_unit" "$UNIT"
    systemctl daemon-reload
fi
rm -f "$new_unit"
systemctl enable drones >/dev/null 2>&1 || true

echo "==> configuration Apache"
new_conf=$(mktemp)
cat > "$new_conf" <<EOF
# Game of Drones (see deploy/setup-drones.sh in the game's repository).
# The browser connects to wss://<host>/drones/ws, forwarded to the world server on the loopback.
<Location "/drones/ws">
	ProxyPass "http://127.0.0.1:$PORT/ws" upgrade=websocket
</Location>
<Directory "$WEB">
	Options -Indexes
	AllowOverride None
	Require all granted
	AddType application/wasm .wasm
</Directory>
EOF
if ! cmp -s "$new_conf" "$CONF"; then
    backup "$CONF"
    install -m 644 "$new_conf" "$CONF"
fi
rm -f "$new_conf"
a2enmod proxy proxy_http >/dev/null
was_enabled=no
[ -e /etc/apache2/conf-enabled/drones.conf ] && was_enabled=yes
a2enconf drones >/dev/null
if ! apache2ctl configtest; then
    echo "!! configuration Apache invalide : retour en arrière" >&2
    [ "$was_enabled" = no ] && a2disconf drones >/dev/null
    latest=$(ls -t "$CONF".bak-* 2>/dev/null | head -n1 || true)
    if [ -n "$latest" ]; then cp -a "$latest" "$CONF"; else rm -f "$CONF"; fi
    exit 1
fi
systemctl reload apache2
echo "==> prêt (la mise en ligne se fait avec deploy/deploy.sh)"
