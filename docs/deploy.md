# Mise en ligne

Dronoids tourne sur la machine Scaleway du mainteneur, à
<https://primo-ideas.xyz/dronoids/>, à côté d'autres sites et jeux. La machine est
préparée une fois, puis chaque mise en ligne ne remplace que le build.

## Ce qu'il y a sur la machine

```text
/home/dronoids/dronoids-server              le serveur de monde (binaire Linux statique)
/home/dronoids/worlds/primo.world           le secteur (JSON), sauvegardé toutes les 30 s et à l'arrêt
/home/dronoids/worlds/primo.accounts.json   pseudo -> jeton secret
/var/www/html/dronoids/                     le client web : index.html, mq_js_bundle.js, god_net.js, le module
/etc/systemd/system/dronoids.service        utilisateur dronoids, port 8082 sur le loopback, 6 colonies IA
/etc/apache2/conf-available/dronoids.conf   activée par a2enconf : le dossier, et le proxy WebSocket
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
vise `ws://127.0.0.1:8082/ws`, ou `DRONOIDS_SERVER`, ou l'adresse saisie dans le menu
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
