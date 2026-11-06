#!/bin/sh
# Installs one build on the machine, run as root from the unpacked archive
# (dronoids-server + web/). Replaces exactly two things:
#
#   /home/dronoids/dronoids-server   renamed over the old binary
#   /var/www/html/dronoids/          the web client, swapped in as a whole
#
# then restarts the world server (which reloads its save) and checks it is
# still up. The machine must have been set up by setup-dronoids.sh.
set -eu

HOME_DIR=/home/dronoids
WEB=/var/www/html/dronoids

for need in /etc/systemd/system/dronoids.service /etc/apache2/conf-enabled/dronoids.conf "$HOME_DIR"; do
    if [ ! -e "$need" ]; then
        echo "!! $need manque : lancez d'abord deploy/setup-dronoids.sh" >&2
        exit 1
    fi
done

install -o dronoids -g dronoids -m 755 dronoids-server "$HOME_DIR/dronoids-server.new"
mv -f "$HOME_DIR/dronoids-server.new" "$HOME_DIR/dronoids-server"

rm -rf "$WEB.new"
cp -r web "$WEB.new"
chmod -R a+rX "$WEB.new"
rm -rf "$WEB.old"
[ -d "$WEB" ] && mv "$WEB" "$WEB.old"
mv "$WEB.new" "$WEB"
rm -rf "$WEB.old"

systemctl restart dronoids
sleep 2
if ! systemctl is-active --quiet dronoids; then
    journalctl -u dronoids -n 30 --no-pager >&2
    echo "!! le serveur ne démarre pas" >&2
    exit 1
fi
journalctl -u dronoids -n 5 --no-pager
echo "==> installé"
