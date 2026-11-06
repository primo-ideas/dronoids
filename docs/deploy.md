# Mise en ligne

Dronoids tourne sur la machine Scaleway du mainteneur, à
<https://primo-ideas.xyz/dronoids/>, à côté d'autres sites et jeux. La machine est
préparée une fois, puis chaque mise en ligne ne remplace que le build.

## Ce qu'il y a sur la machine

```text
/home/dronoids/dronoids-server                 le serveur de monde (binaire Linux statique)
/home/dronoids/worlds/primo.world         le secteur, sauvegardé toutes les 30 s et à l'arrêt
/home/dronoids/worlds/primo.accounts.json pseudo -> jeton secret
/var/www/html/dronoids/                   le client web : index.html, mq_js_bundle.js, god_net.js, le module
/etc/systemd/system/dronoids.service      utilisateur dronoids, port 8082 sur le loopback, 6 colonies IA
/etc/apache2/conf-available/dronoids.conf activée par a2enconf : le dossier, et le proxy WebSocket
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

Le jeu s'appelait d'abord « Game of Drones », servi sur `/drones/`. `setup-dronoids.sh`
a migré cette installation : le secteur a été copié de `/home/drones/worlds`, l'ancien
service et l'ancienne configuration Apache ont été retirés (avec sauvegardes), et
`/drones/` redirige vers `/dronoids/`. Le navigateur reprend aussi ce qu'il avait
stocké sous l'ancien nom (pseudo et jeton).

Le port 8082 est le premier libre : 8081 appartient à Space Race, 3000 et 5432 à un
autre site. Le serveur n'écoute que sur le loopback ; seul Apache, en TLS, le rend
joignable.

## Préparer la machine (une fois)

```bash
scp deploy/setup-dronoids.sh root@primo-ideas.xyz:/tmp/
ssh root@primo-ideas.xyz sh /tmp/setup-dronoids.sh
```

Le script est idempotent. Il crée l'utilisateur, le service et la configuration
Apache ci-dessus, sauvegarde ce qu'il remplace, et teste la configuration Apache
avant de recharger (et la remet en état si le test échoue). Il ne touche ni aux
hôtes virtuels, ni au TLS, ni aux autres sites.

## Mettre en ligne

```bash
./deploy/deploy.sh
```

Il compile le serveur pour `x86_64-unknown-linux-musl` (lié par `rust-lld`, voir
`.cargo/config.toml` : Windows n'a pas de `cc`) et le client pour
`wasm32-unknown-unknown` (réduit par `wasm-opt` s'il est installé), envoie
l'archive, puis `deploy/install.sh` remplace le binaire et le dossier web d'un bloc,
redémarre le service, vérifie qu'il tient debout, et le script récupère la page et
le module par HTTPS.

Un redémarrage déconnecte les joueurs ; ils se reconnectent au même secteur, qui
reprend depuis sa dernière sauvegarde (30 s au plus).

## Le client trouve son serveur

Servie depuis `https://<site>/dronoids/`, la page se connecte à
`wss://<site>/dronoids/ws`. Servie depuis `127.0.0.1` ou `localhost` (développement),
elle vise `ws://127.0.0.1:8082/ws`. `?server=` remplace les deux. Le client bureau
vise `ws://127.0.0.1:8082/ws`, ou `GOD_SERVER`, ou l'adresse saisie dans le menu
« Serveur » (en `ws://` : pas de TLS côté bureau, passer par un tunnel SSH pour la
production).

## Au quotidien

```bash
systemctl status dronoids
journalctl -u dronoids -f
systemctl restart dronoids
```

Repartir d'un secteur neuf : arrêter le service, déplacer `primo.world` et
`primo.accounts.json`, relancer.
