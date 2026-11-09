#!/bin/sh
# One-time (idempotent) setup of the machine for Dronoids, run as root.
# Creates what a deployment then only fills in:
#
#   /home/dronoids/                      the server's home (binary, worlds/)
#   /etc/systemd/system/dronoids.service its own user, loopback port 8082
#   /etc/apache2/conf-available/dronoids.conf   enabled with a2enconf:
#       /dronoids/     -> /var/www/html/dronoids (the web client)
#       /dronoids/ws   -> ws://127.0.0.1:8082/ws (the world server)
#       /drones/...    -> permanent redirect to /dronoids/ (the former name)
#
# It also migrates the former "drones" layout (the game had another name
# until 2026-11-06): the sector is copied over, the old service and
# Apache configuration are switched off (backed up first).
#
# It never touches the virtual hosts, TLS, the document root itself or any
# other game. Anything it overwrites is backed up first, and Apache's
# configuration is tested before it is reloaded (and put back if the test
# fails): a broken configuration would take down every site on the machine.
set -eu

NAME=dronoids
PORT=8082
UNIT=/etc/systemd/system/$NAME.service
CONF=/etc/apache2/conf-available/$NAME.conf
WEB=/var/www/html/$NAME
HOME_DIR=/home/$NAME
stamp=$(date +%Y%m%d-%H%M%S)

backup() {
    if [ -e "$1" ]; then
        cp -a "$1" "$1.bak-$stamp"
        echo "  sauvegarde : $1.bak-$stamp"
    fi
}

echo "==> utilisateur $NAME"
if ! id "$NAME" >/dev/null 2>&1; then
    useradd --system --home-dir "$HOME_DIR" --create-home --shell /usr/sbin/nologin "$NAME"
fi
mkdir -p "$HOME_DIR/worlds" "$WEB"

echo "==> ancienne installation « drones »"
if [ -e /etc/systemd/system/drones.service ]; then
    systemctl disable --now drones >/dev/null 2>&1 || true
    backup /etc/systemd/system/drones.service
    rm -f /etc/systemd/system/drones.service
    systemctl daemon-reload
    echo "  service drones arrêté et retiré"
fi
if [ -d /home/drones/worlds ] && [ ! -e "$HOME_DIR/worlds/primo.world" ]; then
    cp -a /home/drones/worlds/. "$HOME_DIR/worlds/"
    echo "  secteur copié depuis /home/drones/worlds"
fi
chown -R "$NAME:$NAME" "$HOME_DIR"

echo "==> service systemd"
new_unit=$(mktemp)
cat > "$new_unit" <<EOF
# Dronoids world server (see deploy/setup-dronoids.sh in the game's repository).
[Unit]
Description=Dronoids: world server
After=network.target

[Service]
Type=simple
User=$NAME
Group=$NAME
WorkingDirectory=$HOME_DIR
ExecStart=$HOME_DIR/dronoids-server --name primo --bind 127.0.0.1 --port $PORT --bots 6 --level normal --save $HOME_DIR/worlds/primo.world
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
systemctl enable "$NAME" >/dev/null 2>&1 || true

echo "==> configuration Apache"
new_conf=$(mktemp)
cat > "$new_conf" <<EOF
# Dronoids (see deploy/setup-dronoids.sh in the game's repository).
# The browser connects to wss://<host>/$NAME/ws, forwarded to the world server on the loopback.
<Location "/$NAME/ws">
	ProxyPass "http://127.0.0.1:$PORT/ws" upgrade=websocket
</Location>
<Directory "$WEB">
	Options -Indexes
	AllowOverride None
	Require all granted
	AddType application/wasm .wasm
</Directory>
# The game was first published as /drones/.
RedirectMatch permanent "^/drones(/.*)?$" "/$NAME/"
EOF
if ! cmp -s "$new_conf" "$CONF"; then
    backup "$CONF"
    install -m 644 "$new_conf" "$CONF"
fi
rm -f "$new_conf"
a2enmod proxy proxy_http alias >/dev/null
old_enabled=no
[ -e /etc/apache2/conf-enabled/drones.conf ] && old_enabled=yes
was_enabled=no
[ -e "/etc/apache2/conf-enabled/$NAME.conf" ] && was_enabled=yes
[ "$old_enabled" = yes ] && a2disconf drones >/dev/null
a2enconf "$NAME" >/dev/null
if ! apache2ctl configtest; then
    echo "!! configuration Apache invalide : retour en arrière" >&2
    [ "$was_enabled" = no ] && a2disconf "$NAME" >/dev/null
    [ "$old_enabled" = yes ] && a2enconf drones >/dev/null
    latest=$(ls -t "$CONF".bak-* 2>/dev/null | head -n1 || true)
    if [ -n "$latest" ]; then cp -a "$latest" "$CONF"; else rm -f "$CONF"; fi
    exit 1
fi
if [ -e /etc/apache2/conf-available/drones.conf ]; then
    backup /etc/apache2/conf-available/drones.conf
    rm -f /etc/apache2/conf-available/drones.conf
fi
systemctl reload apache2
if [ -d /var/www/html/drones ]; then
    rm -rf /var/www/html/drones
    echo "  ancien client web /var/www/html/drones retiré (remplacé par la redirection)"
fi
echo "==> prêt (la mise en ligne se fait avec deploy/deploy.sh)"
