# Mise en ligne

Dronoids tourne sur la machine Scaleway du mainteneur, à
<https://primo-ideas.xyz/dronoids/>, à côté d'autres sites et jeux. La machine est
préparée une fois, puis chaque mise en ligne ne remplace que le build.

## Ce qu'il y a sur la machine

```text
/home/dronoids/dronoids-server              le serveur de monde (binaire Linux statique)
/home/dronoids/worlds/primo.world           le secteur (JSON), sauvegardé toutes les 30 s et à l'arrêt
/home/dronoids/worlds/primo.accounts.json   pseudo -> jeton secret d'invité, ou "site" (tenu par un compte)
/home/dronoids/site/dronoids_site.py        le service des comptes (Python, bibliothèque standard)
/home/dronoids/site/config.json             ses réglages, dont les clés de connexion (écrit une fois)
/home/dronoids/site/accounts.db             les comptes (SQLite) : pseudo et identifiants des services
/home/dronoids/site/secret                  secret partagé : le site signe les tickets, le serveur les vérifie
/var/www/html/dronoids/                     le site (index.html, site.css, site.js) et le jeu dans jouer/
/etc/systemd/system/dronoids.service        utilisateur dronoids, port 8082 sur le loopback, 6 colonies IA
/etc/systemd/system/dronoids-site.service   le service des comptes, port 8083 sur le loopback
/etc/apache2/conf-available/dronoids.conf   activée par a2enconf : le dossier, le proxy WebSocket et /dronoids/api/
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

Le jeu a d'abord été servi sur `/drones/`. Sur une machine
qui porte encore cette installation, `setup-dronoids.sh` la migre : il arrête l'ancien
service, copie le secteur de `/home/drones/worlds`, retire l'ancien service et
l'ancienne configuration Apache (avec sauvegardes), et `/drones/` redirige ensuite vers
`/dronoids/`. Le navigateur reprend aussi ce qu'il avait stocké sous l'ancien nom
(pseudo et jeton). Le jeu est indisponible entre ce script et la mise en ligne qui suit.

Le port 8082 est le premier libre : 8081 appartient à Space Race, 3000 et 5432 à un
autre site. Le serveur n'écoute que sur le loopback ; seul Apache, en TLS, le rend
joignable.

## La sauvegarde du secteur

Le monde est généré à la demande à partir de sa graine : le fichier ne contient que
ce que les joueurs ont changé (usines, dronoids, cartes explorées, gisements entamés).
Le carré de 768 unités des premières versions reste au centre du secteur `primo`, tel
qu'il était ; tout autour, le terrain est calculé.

Depuis le protocole 5, `primo.world` est en JSON : une version suivante peut ajouter
des champs (avec une valeur par défaut) et relire le secteur. Le serveur relit aussi
les sauvegardes bincode des versions précédentes (protocoles 3 et 4) ; il en garde
alors l'original dans `primo.old.world` avant de l'écraser au nouveau format. Les
joueurs d'un secteur au protocole 3, d'avant le brouillard, retrouvent une carte de
ce que voient leurs usines et leurs dronoids. Une sauvegarde illisible n'est jamais
écrasée : elle est mise de côté (`primo.unreadable-<horodatage>.world`) et le serveur part
d'un secteur neuf, ce que son journal signale.

## Préparer la machine (une fois)

```bash
scp deploy/setup-dronoids.sh root@primo-ideas.xyz:/tmp/
ssh root@primo-ideas.xyz sh /tmp/setup-dronoids.sh
```

Le script est idempotent. Il crée l'utilisateur, le service et la configuration
Apache ci-dessus, sauvegarde ce qu'il remplace, et teste la configuration Apache
avant de recharger (et la remet en état si le test échoue). Il ne touche ni aux
hôtes virtuels, ni au TLS, ni aux autres sites.

## Le site et les comptes

`https://primo-ideas.xyz/dronoids/` est le site ; le jeu est dans `jouer/`. On joue
sans compte, comme invité : le pseudo est alors protégé par un jeton gardé dans le
navigateur. Un compte se crée avec Google, Discord, GitHub ou Twitch ; on n'en garde
que le pseudo et l'identifiant opaque que le service donne pour la personne (ni
e-mail, ni nom, ni jeton d'accès).

Quand on est connecté, la page du jeu demande au site (`api/me`) le pseudo et un
ticket signé, valable 7 jours, que le jeu présente au serveur de monde à la place du
jeton d'invité. Le serveur vérifie la signature avec le secret partagé
(`--site-secret`), sans jamais interroger le site, et le nom appartient dès lors au
compte : plus aucun jeton d'invité ne l'ouvre. Un invité qui crée son compte depuis
le même navigateur garde son pseudo et sa colonie : le site lit son jeton et le
compare à `primo.accounts.json`.

### Activer la connexion par un service

Le site ne propose que les services dont les clés sont renseignées. Pour chacun,
créer une application chez le service avec, pour adresse de retour :

```text
https://primo-ideas.xyz/dronoids/api/callback/google    console.cloud.google.com → API et services → Identifiants → ID client OAuth (application Web)
https://primo-ideas.xyz/dronoids/api/callback/discord   discord.com/developers/applications → OAuth2
https://primo-ideas.xyz/dronoids/api/callback/github    github.com/settings/developers → OAuth Apps
https://primo-ideas.xyz/dronoids/api/callback/twitch    dev.twitch.tv/console/apps
```

puis recopier l'identifiant et le secret de l'application dans
`/home/dronoids/site/config.json` (`providers` → `client_id`, `client_secret`) et
relancer le service :

```bash
systemctl restart dronoids-site
```

Le fichier n'est écrit qu'une fois par `setup-dronoids.sh` : les clés y restent d'une
mise en ligne à l'autre.

### Essayer le site sur sa machine

```bash
./deploy/build-web.sh
python site/dronoids_site.py --config site/dev-config.json
./target/release/dronoids-server --port 8082 --save target/site-dev/primo.world --site-secret target/site-dev/secret
```

puis <http://localhost:8083/dronoids/>. En local seulement, un « compte d'essai » se
connecte sans aucun service. Les tests : `python site/test_site.py`.

## Mettre en ligne

```bash
./deploy/deploy.sh
```

Il compile le serveur pour `x86_64-unknown-linux-musl` (lié par `rust-lld`, voir
`.cargo/config.toml` : Windows n'a pas de `cc`) et le client pour
`wasm32-unknown-unknown` (réduit par `wasm-opt` s'il est installé), envoie
l'archive, puis `deploy/install.sh` remplace le binaire, le service des comptes et le
dossier web d'un bloc, redémarre les deux services, vérifie qu'ils tiennent debout,
et le script récupère le site, le jeu, le module et `api/me` par HTTPS.

Quand `setup-dronoids.sh` change (comme à l'arrivée du site), le relancer avant la
mise en ligne.

Un redémarrage déconnecte les joueurs ; ils se reconnectent au même secteur, qui
reprend depuis sa dernière sauvegarde (30 s au plus).

## Le client trouve son serveur

Servie depuis `https://<site>/dronoids/jouer/`, la page se connecte à
`wss://<site>/dronoids/ws`. Servie depuis `127.0.0.1` ou `localhost` (développement),
elle vise `ws://127.0.0.1:8082/ws`. `?server=` remplace les deux. Le client bureau
vise `ws://127.0.0.1:8082/ws`, ou `DRONOIDS_SERVER`, ou l'adresse saisie dans le menu
« Serveur » (en `ws://` : pas de TLS côté bureau, passer par un tunnel SSH pour la
production).

## Au quotidien

```bash
systemctl status dronoids dronoids-site
journalctl -u dronoids -f
journalctl -u dronoids-site -f
systemctl restart dronoids
```

Repartir d'un secteur neuf : arrêter le service, déplacer `primo.world` et
`primo.accounts.json`, relancer.
