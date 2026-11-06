# Mise en ligne

Game of Drones tourne sur la machine Scaleway du mainteneur, à
<https://primo-ideas.xyz/drones/>, à côté d'autres sites et jeux. La machine est
préparée une fois, puis chaque mise en ligne ne remplace que le build.

## Ce qu'il y a sur la machine

```text
/home/drones/god-server                 le serveur de monde (binaire Linux statique)
/home/drones/worlds/primo.world         le secteur, sauvegardé toutes les 30 s et à l'arrêt
/home/drones/worlds/primo.accounts.json pseudo -> jeton secret
/var/www/html/drones/                   le client web : index.html, mq_js_bundle.js, god_net.js, le module
/etc/systemd/system/drones.service      utilisateur drones, port 8082 sur le loopback, 6 colonies IA
/etc/apache2/conf-available/drones.conf activée par a2enconf : le dossier, et le proxy WebSocket
```

```apache
<Location "/drones/ws">
	ProxyPass "http://127.0.0.1:8082/ws" upgrade=websocket
</Location>
<Directory "/var/www/html/drones">
	Options -Indexes
	AllowOverride None
	Require all granted
	AddType application/wasm .wasm
</Directory>
```

Le port 8082 est le premier libre : 8081 appartient à Space Race, 3000 et 5432 à un
autre site. Le serveur n'écoute que sur le loopback ; seul Apache, en TLS, le rend
joignable.

## Préparer la machine (une fois)

```bash
scp deploy/setup-drones.sh root@primo-ideas.xyz:/tmp/
ssh root@primo-ideas.xyz sh /tmp/setup-drones.sh
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

Servie depuis `https://<site>/drones/`, la page se connecte à
`wss://<site>/drones/ws`. Servie depuis `127.0.0.1` ou `localhost` (développement),
elle vise `ws://127.0.0.1:8082/ws`. `?server=` remplace les deux. Le client bureau
vise `ws://127.0.0.1:8082/ws`, ou `GOD_SERVER`, ou l'adresse saisie dans le menu
« Serveur » (en `ws://` : pas de TLS côté bureau, passer par un tunnel SSH pour la
production).

## Au quotidien

```bash
systemctl status drones
journalctl -u drones -f
systemctl restart drones
```

Repartir d'un secteur neuf : arrêter le service, déplacer `primo.world` et
`primo.accounts.json`, relancer.
