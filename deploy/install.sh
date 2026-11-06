#!/bin/sh
# Installs one build on the machine, run as root from the unpacked archive
# (god-server + web/). Replaces exactly two things:
#
#   /home/drones/god-server     renamed over the old binary
#   /var/www/html/drones/       the web client, swapped in as a whole
#
# then restarts the world server (which reloads its save) and checks it is
# still up. The machine must have been set up by setup-drones.sh.
set -eu

HOME_DIR=/home/drones
WEB=/var/www/html/drones

for need in /etc/systemd/system/drones.service /etc/apache2/conf-enabled/drones.conf "$HOME_DIR"; do
    if [ ! -e "$need" ]; then
        echo "!! $need manque : lancez d'abord deploy/setup-drones.sh" >&2
        exit 1
    fi
done

install -o drones -g drones -m 755 god-server "$HOME_DIR/god-server.new"
mv -f "$HOME_DIR/god-server.new" "$HOME_DIR/god-server"

rm -rf "$WEB.new"
cp -r web "$WEB.new"
chmod -R a+rX "$WEB.new"
rm -rf "$WEB.old"
[ -d "$WEB" ] && mv "$WEB" "$WEB.old"
mv "$WEB.new" "$WEB"
rm -rf "$WEB.old"

systemctl restart drones
sleep 2
if ! systemctl is-active --quiet drones; then
    journalctl -u drones -n 30 --no-pager >&2
    echo "!! le serveur ne démarre pas" >&2
    exit 1
fi
journalctl -u drones -n 5 --no-pager
echo "==> installé"
