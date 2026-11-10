#!/bin/sh
# Installs one build on the machine, run as root from the unpacked archive
# (dronoids-server, dronoids_site.py, web/). Replaces exactly three things:
#
#   /home/dronoids/dronoids-server        renamed over the old binary
#   /home/dronoids/site/dronoids_site.py  the account service
#   /var/www/html/dronoids/               the site and its game, swapped in as a whole
#
# then restarts both services (the world server reloads its save) and
# checks they are still up. The machine must have been set up by
# setup-dronoids.sh.
set -eu

HOME_DIR=/home/dronoids
WEB=/var/www/html/dronoids

for need in /etc/systemd/system/dronoids.service /etc/systemd/system/dronoids-site.service /etc/apache2/conf-enabled/dronoids.conf "$HOME_DIR/site"; do
    if [ ! -e "$need" ]; then
        echo "!! $need manque : lancez d'abord deploy/setup-dronoids.sh" >&2
        exit 1
    fi
done

install -o dronoids -g dronoids -m 755 dronoids-server "$HOME_DIR/dronoids-server.new"
mv -f "$HOME_DIR/dronoids-server.new" "$HOME_DIR/dronoids-server"
install -o dronoids -g dronoids -m 644 dronoids_site.py "$HOME_DIR/site/dronoids_site.py.new"
mv -f "$HOME_DIR/site/dronoids_site.py.new" "$HOME_DIR/site/dronoids_site.py"

rm -rf "$WEB.new"
cp -r web "$WEB.new"
chmod -R a+rX "$WEB.new"
rm -rf "$WEB.old"
[ -d "$WEB" ] && mv "$WEB" "$WEB.old"
mv "$WEB.new" "$WEB"
rm -rf "$WEB.old"

systemctl restart dronoids-site dronoids
sleep 2
for unit in dronoids-site dronoids; do
    if ! systemctl is-active --quiet "$unit"; then
        journalctl -u "$unit" -n 30 --no-pager >&2
        echo "!! $unit ne démarre pas" >&2
        exit 1
    fi
done
journalctl -u dronoids-site -n 2 --no-pager
journalctl -u dronoids -n 5 --no-pager
echo "==> installé"
