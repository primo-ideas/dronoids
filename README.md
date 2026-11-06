# Dronoids

Un secteur spatial partagé et persistant, vue du dessus, géométrique et néon.
On y arrive **sans rien**, sinon de quoi bâtir une usine. L'usine fabrique des
**drones**, qui suivent à la lettre un **programme** écrit dans un petit assembleur
ludique, **DASM**. Les programmes fournis sont volontairement naïfs : le jeu, c'est
d'en écrire de meilleurs. Votre colonie continue de travailler quand vous partez.

**Jouer : <https://primo-ideas.xyz/dronoids/>**

Inspiré de *Human Resource Machine* pour le langage et le débogueur pas à pas.

## Lancer en local

```bash
cargo run --release                                   # le jeu (bureau)
cargo run --release --bin dronoids-server                  # un secteur local, ws://127.0.0.1:8082/ws
./deploy/build-web.sh --serve                         # la version navigateur, http://127.0.0.1:8090
```

Rust stable récent (édition 2024). Dépendances : `macroquad` (rendu), `serde`,
`bincode`, `serde_json`, et `tungstenite` côté bureau/serveur.

## Le secteur

- Un espace continu de 768 × 768 unités : ceintures d'astéroïdes, champs de
  cristaux qui **repoussent** ailleurs à mesure qu'on les épuise, et des colonies IA
  qui vivent leur vie.
- **Arrivée** : 170 cristaux, rien d'autre. On choisit où poser sa première usine
  (loin des autres colonies). Un **bouclier** protège les nouveaux venus 10 minutes :
  ils ne peuvent ni être attaqués, ni attaquer.
- **Brouillard de guerre** : on ne voit qu'autour de ses usines (18 unités) et de
  ses drones (12). L'espace déjà exploré reste en mémoire, voilé (astéroïdes et
  cristaux, mais aucune unité étrangère) ; l'inconnu reste noir. La carte explorée
  est propre à chaque joueur et persiste : vos drones cartographient aussi pendant
  votre absence. (Chaque client rejoue tout le monde : le brouillard est un choix
  d'affichage, pas une protection.)
- **Rythme lent** : 4 ticks par seconde ; un pas prend 1 s, extraire un cristal 2 s,
  une usine produit un drone toutes les 30 s.
- Les IA ne lancent de raids qu'autour de chez elles : s'installer loin d'elles,
  c'est grandir en paix.
- Votre pseudo est protégé par un jeton secret gardé par votre navigateur (ou
  `./save/` sur le bureau).
- **Entraînement hors ligne** : un secteur privé avec des IA, pause et vitesse x8.

## Contrôles

| Touche | Effet |
| --- | --- |
| Clic gauche | sélectionner ; double-clic sur une usine pour son programme |
| Clic droit glissé, molette | déplacer la caméra, zoomer |
| ZQSD / WASD / flèches | déplacer la caméra |
| `B` | nouvelle usine (Maj+clic pour en poser plusieurs) |
| `E` / `P` | programme de l'usine, débogueur du drone / tous les programmes |
| `T` | poser la cible de l'usine (lue par `chasseur`) |
| `M`, `L`, `H`, `Tab` | carte, colonies, retour à la base, usine suivante |
| `F1` | aide du langage |
| Hors ligne | `Espace` pause, `1`-`4` vitesse, `N` tick suivant |

L'écran reste volontairement épuré : une ligne d'état en haut à gauche, un dock en
bas, et une seule ligne d'objectif quand elle est utile. Carte et colonies
s'ouvrent à la demande.

## DASM en bref

```asm
; MINEUR -- le programme de départ
debut:
    CARGO a
    JGE a, FULL, rentrer
    DIST a, ORE          ; distance au cristal le plus proche
    JEQ a, NONE, errer   ; rien en vue ?
    JLE a, 1, creuser    ; à portée : on creuse
    SCAN b, ORE          ; b = angle vers le cristal
    MOVE b
    JZ ok, esquive       ; bloqué par un astéroïde
    JMP debut
...
```

- Registres `a b c d`, plus `ok` (1 si la dernière action a réussi).
- Angles en degrés, sens horaire : `N=0 E=90 S=180 W=270`, `NONE=-1`.
  Choses : `EMPTY WALL ORE ALLY ENEMY BASE EBASE`, `FULL` = capacité de la soute.
- **Données** `SET ADD SUB MUL DIV MOD RAND INC DEC ANGLE` · **Sauts** `JMP JZ JNZ
  JEQ JNE JLT JGT JLE JGE CALL RET` · **Capteurs** `SCAN DIST DX DY COUNT LOOK CARGO
  HP POSX POSY PEEK POKE` · **Actions** `MOVE angle`, `MINE`, `DROP`, `ATTACK`, `WAIT`.
- `MINE`/`DROP` quand `DIST ≤ 1`, `ATTACK` quand `DIST ≤ 2`. Capteurs à 12 unités,
  usines ennemies à 40, les vôtres partout. 16 cases de mémoire de ruche par usine.

[examples/mineur_plus.dasm](examples/mineur_plus.dasm) regarde devant lui (`LOOK`)
et part explorer en ligne droite : un test vérifie qu'il extrait nettement plus
que le mineur de base sur la durée (+20 % au bout d'une demi-heure).

## Comment marche le multijoueur

Le serveur (`dronoids-server`) possède le monde de référence et fait tourner les IA.
À chaque tick, il diffuse la liste ordonnée des commandes des joueurs ; chaque
client rejoue ces commandes sur sa propre copie. La simulation est entièrement
déterministe (entiers en virgule fixe, table de sinus, `atan2` et racines
entières, RNG maison), donc seules les commandes transitent, jamais les milliers de
drones. Un arrivant reçoit un instantané du monde (les astéroïdes sont recréés à
partir de la graine), puis le flux de commandes. Les clients envoient un hash
régulier ; un client qui diverge, ou qui a pris trop de retard (onglet en
arrière-plan), reçoit un nouvel instantané.

Tout passe par WebSocket (messages bincode) : `tungstenite` côté serveur et bureau,
et dans le navigateur la WebSocket de la page, via un petit plugin miniquad
([web/god_net.js](web/god_net.js)) qui fournit aussi le stockage local.

## Mise en ligne

Voir [docs/deploy.md](docs/deploy.md). En bref : `./deploy/deploy.sh` compile le
serveur pour Linux (musl statique, lié par `rust-lld`) et le client WASM, envoie le
tout et l'installe sur la machine.

## Architecture

La bibliothèque (sans graphismes, testable) :

- `src/vm.rs` : DASM, assembleur, interpréteur, coloration syntaxique.
- `src/geo.rs` : géométrie entière déterministe.
- `src/sim/` : le monde (atterrissage, bouclier, repousse des cristaux), la carte,
  l'index spatial.
- `src/ai.rs` : les colonies IA, qui jouent via les mêmes commandes qu'un humain.
- `src/server.rs`, `src/net.rs` : serveur de monde, protocole WebSocket.
- `src/bin/server.rs` : `dronoids-server`.

Le jeu : rendu néon avec bloom (`gfx.rs`), monde et particules (`view.rs`,
`fx.rs`), interface (`ui.rs`, `game.rs`, `menu.rs`), éditeur (`editor.rs`),
différences bureau/navigateur (`platform.rs`, `library.rs`).

```bash
cargo test
cargo test --release --lib bench_world -- --ignored --nocapture   # perfs et rythme des IA
```

Police : Fira Mono (SIL Open Font License, voir `assets/fonts`).
