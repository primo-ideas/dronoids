# Game of Drones

Jeu de stratégie et de programmation dans un grand monde spatial, vue du dessus,
géométrique et néon. On pose des **usines** qui fabriquent des **drones** ; chaque
usine exécute un **programme** écrit dans un petit assembleur ludique, **DASM**,
que ses drones suivent à la lettre. Les programmes fournis sont volontairement
naïfs : le jeu, c'est d'en écrire de meilleurs.

Inspiré de *Human Resource Machine* pour le langage et le débogueur pas à pas.

## Lancer

```bash
cargo run --release
```

Serveur de monde dédié, sans fenêtre :

```bash
cargo run --release --bin god-server -- --name nebuleuse --bots 4
```

Rust stable récent (édition 2024). Dépendances : `macroquad` (rendu), `serde`,
`bincode` et `serde_json` (réseau, sauvegardes).

## Le monde

- Un espace continu de 512 × 512 unités : ceintures d'astéroïdes, champs de
  cristaux (plus riches loin des points de départ), et 16 emplacements de départ
  très éloignés les uns des autres.
- Les drones se déplacent librement, à n'importe quel angle ; les usines se posent
  n'importe où (à moins de 8 unités d'une de vos unités, loin des usines ennemies).
- Le rythme est lent : un pas prend 0,6 s, extraire un cristal 1,2 s, une usine
  produit un drone toutes les 10 s. En solo, la vitesse va de la pause à x8.
- **Solo** : vous apparaissez dans un grand monde avec 0 à 8 IA lointaines
  (3 niveaux). Une IA « Normal » n'attaque qu'au bout de 10 minutes, et ses
  chasseurs mettent ensuite du temps à traverser le monde.
- **Multijoueur** : le menu *Multijoueur* liste les mondes hébergés sur le réseau
  local, permet d'en rejoindre un par son adresse, ou d'héberger le vôtre (avec ou
  sans IA). On peut rejoindre un monde en cours de partie ; si vous êtes éliminé,
  vous pouvez réapparaître ailleurs.
- Les mondes hébergés sont sauvegardés dans `./worlds` : héberger à nouveau le même
  nom reprend la partie. Vos drones continuent de travailler pendant que vous êtes
  déconnecté.

## Contrôles

| Touche | Effet |
| --- | --- |
| Clic gauche | sélectionner usine / drone ; double-clic sur une usine pour éditer son programme |
| Clic droit glissé, molette | déplacer la caméra, zoomer |
| ZQSD / WASD / flèches | déplacer la caméra |
| `B` | poser une usine (Maj+clic pour en poser plusieurs) |
| `T` | poser la **cible** de l'usine sélectionnée (lue par le programme `chasseur`) |
| `E` | éditer le programme de l'usine / déboguer le drone sélectionné |
| `P` | ouvrir l'éditeur de programmes |
| `Espace`, `1`-`4`, `N` | pause, vitesse x1 à x8, tick suivant (solo) |
| `H`, `Tab` | revenir à la base, passer d'une usine à l'autre |
| `F1` | aide complète du langage |
| Éditeur | `Ctrl+S` appliquer, `Ctrl+Z`/`Ctrl+Y`, `Ctrl+C/X/V`, `Ctrl+D` dupliquer la ligne, `Tab`/`Maj+Tab` |

« Appliquer » sauvegarde le programme dans `./programs/<nom>.dasm`, puis met à
jour toutes vos usines qui l'utilisent **et leurs drones déjà en vol**. Le
débogueur (bouton *Déboguer* sur un drone) montre ses registres, son cap, sa
position, la mémoire de ruche de son usine, et surligne l'action en cours.

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

- Registres `a b c d`, plus `ok` (lecture seule, 1 si la dernière action a réussi).
- Angles en degrés, sens horaire : `N=0 E=90 S=180 W=270`, `NONE=-1`.
  Choses : `EMPTY WALL ORE ALLY ENEMY BASE EBASE`, `FULL` = capacité de la soute.
- **Données** : `SET ADD SUB MUL DIV MOD RAND INC DEC ANGLE`
- **Sauts** : `JMP JZ JNZ JEQ JNE JLT JGT JLE JGE CALL RET`
- **Capteurs** (instantanés) : `SCAN DIST DX DY COUNT LOOK CARGO HP POSX POSY`, et
  la mémoire de ruche partagée par les drones d'une usine, `PEEK POKE` (16 cases).
- **Actions** (prennent du temps) : `MOVE angle`, `MINE`, `DROP`, `ATTACK`, `WAIT`.
- Portées : `MINE`/`DROP` quand `DIST ≤ 1`, `ATTACK` quand `DIST ≤ 2`. Les capteurs
  voient à 12 unités, les usines ennemies à 40, les vôtres partout.
- Au plus 32 instructions par décision, sinon le drone « calcule » un instant. La
  fin du programme reboucle au début.

`SCAN` donne la direction directe et fonce dans les astéroïdes : à vous de faire
mieux. [examples/mineur_plus.dasm](examples/mineur_plus.dasm) regarde devant lui
(`LOOK`) et contourne les obstacles ; un test vérifie qu'il extrait plus que le
mineur de base. Il reste beaucoup de marge : embouteillages devant l'usine,
partage des gisements via la mémoire de ruche, escortes, expéditions vers les
champs lointains...

Les programmes `./programs` écrits pour l'ancienne version en grille sont mis de
côté sous `<nom>-v1.dasm` au premier lancement.

## Multijoueur : comment ça marche

Le serveur (dans le jeu quand on héberge, ou `god-server`) possède le monde de
référence et fait tourner les IA. À chaque tick, il diffuse la liste ordonnée des
commandes des joueurs ; chaque client rejoue ces commandes sur sa propre copie.
La simulation est entièrement déterministe (entiers en virgule fixe, table de
sinus, `atan2` et racines entières, RNG maison), donc seules les commandes
transitent, jamais les milliers de drones. Un nouvel arrivant reçoit un instantané
du monde puis le flux de commandes. Les clients envoient régulièrement un hash de
leur monde ; un client qui diverge reçoit un nouvel instantané.

Ports : TCP 7777 (jeu), UDP 7778 (découverte des mondes sur le réseau local).

## Options de lancement

`--solo [easy|normal|hard]`, `--ais N`, `--sandbox`, `--seed N`, `--ticks N` (avance
rapide), `--zoom Z`, `--editor`, `--debug-drone`, `--help-screen`, `--build`,
`--host MONDE`, `--join ADRESSE`, `--name PSEUDO`, `--worlds`,
`--screenshot out.png [--after SECONDES | --frames N]` (capture puis quitte).

`god-server [--name NOM] [--port 7777] [--bots 4] [--level easy|normal|hard] [--seed N] [--save FICHIER]`

## Architecture

La bibliothèque (sans graphismes, testable) :

- `src/vm.rs` : DASM, assembleur (erreurs par ligne), interpréteur, coloration syntaxique.
- `src/geo.rs` : géométrie entière déterministe (angles, sinus, atan2, distances).
- `src/sim/` : monde continu, astéroïdes, cristaux, usines, drones, actions qui durent ;
  génération du monde ; index spatial.
- `src/ai.rs` : IA (3 niveaux) qui joue via les mêmes commandes qu'un humain.
- `src/server.rs`, `src/net.rs` : serveur de monde, protocole, découverte LAN.
- `src/programs.rs` : programmes fournis.
- `src/bin/server.rs` : serveur dédié `god-server`.

Le jeu (`src/main.rs` et voisins) : rendu néon avec bloom multi-passes (`gfx.rs`),
monde, étoiles et particules (`view.rs`, `fx.rs`), interface immédiate (`ui.rs`),
éditeur de code (`editor.rs`), partie en cours (`game.rs`), menus et navigateur de
mondes (`menu.rs`).

```bash
cargo test
cargo test --release --lib bench_world -- --ignored --nocapture   # perfs et rythme des IA
```

## Pistes

Brouillard de guerre, construction d'usines par les drones (`BUILD`), échanges
entre drones (`GIVE`), messages radio entre usines, sauvegarde des parties solo,
liste de mondes sur Internet (serveur annuaire), sons.
