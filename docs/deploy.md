# Going online

Dronoids runs on the maintainer's Scaleway machine, at
<https://primo-ideas.xyz/dronoids/>, beside other sites and games. The machine is
prepared once, and then each deployment only replaces the build.

## What is on the machine

```text
/home/dronoids/dronoids-server              the world server (static Linux binary)
/home/dronoids/worlds/primo.world           the sector (JSON), saved every 30 s and on shutdown
/home/dronoids/worlds/primo.accounts.json   nickname -> a guest's secret token, or "site" (held by an account)
/home/dronoids/site/dronoids_site.py        the account service (Python, standard library)
/home/dronoids/site/config.json             its settings, the sign-in keys among them (written once)
/home/dronoids/site/accounts.db             the accounts (SQLite): nicknames and the services' identifiers
/home/dronoids/site/secret                  shared secret: the site signs the tickets, the server checks them
/var/www/html/dronoids/                     the site (index.html, site.css, site.js) and the game in play/
/etc/systemd/system/dronoids.service        user dronoids, port 8082 on the loopback, 6 AI colonies
/etc/systemd/system/dronoids-site.service   the account service, port 8083 on the loopback
/etc/apache2/conf-available/dronoids.conf   enabled with a2enconf: the directory, the WebSocket proxy and /dronoids/api/
```

```apache
<Location "/dronoids/ws">
	ProxyPass "http://127.0.0.1:8082/ws" upgrade=websocket
</Location>
<Directory "/var/www/html/dronoids">
	Options -Indexes
	AllowOverride None
	Require all granted
	AddType application/wasm .wasm
</Directory>
# The game was first published as /drones/.
RedirectMatch permanent "^/drones(/.*)?$" "/dronoids/"
```

The game was first served at `/drones/`. On a machine that still carries that
install, `setup-dronoids.sh` migrates it: it stops the old service, copies the sector
from `/home/drones/worlds`, removes the old service and the old Apache configuration
(with backups), and `/drones/` then redirects to `/dronoids/`. The browser also takes
back what it had stored under the old name (nickname and token). The game is
unavailable between that script and the deployment that follows.

Port 8082 is the first free one: 8081 belongs to Space Race, 3000 and 5432 to another
site. The server only listens on the loopback; only Apache, over TLS, makes it
reachable.

## The sector's save

The world is generated on demand from its seed: the file only holds what the players
changed (factories, dronoids, explored maps, deposits that have been mined). The
768-unit square of the first versions stays at the center of the `primo` sector, as it
was; all around it, the terrain is computed.

Since protocol 5, `primo.world` is JSON: a later version can add fields (with a
default value) and read the sector back. The server also reads the bincode saves of
earlier versions (protocols 3 and 4); it then keeps the original in `primo.old.world`
before overwriting it in the new format. The players of a protocol-3 sector, from
before the fog, get a map of what their factories and dronoids see. An unreadable save
is never overwritten: it is set aside (`primo.unreadable-<timestamp>.world`) and the
server starts from a new sector, which its log says.

The game was in French until protocol 7. A sector saved before then is brought up to
date when it is loaded: the built-in programs take their English names (`mineur`,
`eclaireur`, `chasseur` and `gardien` become `miner`, `scout`, `hunter` and `guard`),
untouched copies of them their English wording, with the same instructions line for
line so that running dronoids carry on, and the AI colonies become `AI <star>`
instead of `IA <star>`. Players' own programs keep their code. The players' browsers
do the same with the programs they keep.

## Preparing the machine (once)

```bash
scp deploy/setup-dronoids.sh root@primo-ideas.xyz:/tmp/
ssh root@primo-ideas.xyz sh /tmp/setup-dronoids.sh
```

The script is idempotent. It creates the user, the service and the Apache
configuration above, backs up what it replaces, and tests the Apache configuration
before reloading it (and puts it back if the test fails). It touches neither the
virtual hosts, nor TLS, nor the other sites.

## The site and the accounts

`https://primo-ideas.xyz/dronoids/` is the site; the game is in `play/`. Its first
address, `jouer/`, is a page that sends visitors on to `play/`. Playing needs no
account, as a guest: the nickname is then protected by a token kept in the browser.
An account is made with Google, Discord, GitHub or Twitch; all that is kept is the
nickname and the opaque identifier the service gives for the person (no email, no
name, no access token).

When signed in, the game's page asks the site (`api/me`) for the nickname and a signed
ticket, valid for 7 days, which the game shows the world server instead of the guest
token. The server checks the signature with the shared secret (`--site-secret`),
without ever asking the site, and the name then belongs to the account: no guest
token opens it anymore. A guest who creates an account from the same browser keeps
their nickname and their colony: the site reads their token and checks it against
`primo.accounts.json`.

The database's columns were named in French until the site was translated: the
service renames them (`pseudo` to `nickname`) the first time it starts on such a
database.

### Turning on sign-in with a service

The site only offers the services whose keys are filled in. For each one, create an
application at the service with, as its redirect address:

```text
https://primo-ideas.xyz/dronoids/api/callback/google    console.cloud.google.com → APIs & Services → Credentials → OAuth client ID (web application)
https://primo-ideas.xyz/dronoids/api/callback/discord   discord.com/developers/applications → OAuth2
https://primo-ideas.xyz/dronoids/api/callback/github    github.com/settings/developers → OAuth Apps
https://primo-ideas.xyz/dronoids/api/callback/twitch    dev.twitch.tv/console/apps
```

then copy the application's identifier and secret into
`/home/dronoids/site/config.json` (`providers` → `client_id`, `client_secret`) and
restart the service:

```bash
systemctl restart dronoids-site
```

`setup-dronoids.sh` only writes that file once: the keys stay there from one
deployment to the next.

### Trying the site on one's own machine

```bash
./deploy/build-web.sh
python site/dronoids_site.py --config site/dev-config.json
./target/release/dronoids-server --port 8082 --save target/site-dev/primo.world --site-secret target/site-dev/secret
```

then <http://localhost:8083/dronoids/>. Locally only, a "test account" signs in
without any service. The tests: `python site/test_site.py`.

## Deploying

```bash
./deploy/deploy.sh
```

It builds the server for `x86_64-unknown-linux-musl` (linked with `rust-lld`, see
`.cargo/config.toml`: Windows has no `cc`) and the client for
`wasm32-unknown-unknown` (shrunk by `wasm-opt` if it is installed), uploads the
archive, then `deploy/install.sh` replaces the binary, the account service and the web
directory in one go, restarts both services and checks that they stay up, and the
script fetches the site, the game, the module and `api/me` over HTTPS.

When `setup-dronoids.sh` changes (as when the site arrived), run it again before
deploying.

A restart disconnects the players; they connect again to the same sector, which
resumes from its last save (30 s at most).

## How the client finds its server

Served from `https://<site>/dronoids/play/`, the page connects to
`wss://<site>/dronoids/ws`. Served from `127.0.0.1` or `localhost` (development), it
aims at `ws://127.0.0.1:8082/ws`. `?server=` overrides both. The desktop client aims
at `ws://127.0.0.1:8082/ws`, or `DRONOIDS_SERVER`, or the address typed into the
"Server" menu (over `ws://`: no TLS on the desktop, go through an SSH tunnel for
production).

## Day to day

```bash
systemctl status dronoids dronoids-site
journalctl -u dronoids -f
journalctl -u dronoids-site -f
systemctl restart dronoids
```

Starting from a new sector: stop the service, move `primo.world` and
`primo.accounts.json` away, start it again.
