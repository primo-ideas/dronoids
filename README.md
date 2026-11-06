# Game of Drones

Jeu de stratégie et de programmation, vue du dessus, géométrique et néon.
On pose des **usines** qui fabriquent des **drones** ; chaque usine exécute un
**programme** écrit dans un petit assembleur ludique, **DASM**, que les drones
suivent à la lettre. Les programmes fournis sont volontairement naïfs : le jeu,
c'est d'en écrire de meilleurs.

Inspiré de *Human Resource Machine* pour le langage et le débogueur pas à pas.

## Lancer

```bash
cargo run --release
```

Rust stable récent (édition 2024). Une seule dépendance graphique : `macroquad`.

## Principe

- Vous démarrez avec une usine qui fabrique des drones (10 minerai pièce, 8 drones par usine par défaut, quota réglable de 0 à 24).
- Les drones minent les cristaux ambrés et les rapportent à une usine alliée.
- Une nouvelle usine coûte 120 minerai et doit être posée à moins de 6 cases d'une de vos unités (et loin des usines ennemies).
- Un joueur sans usine, qui ne peut plus en reconstruire, est éliminé.
- Chaque tick (8 par seconde en x1), chaque drone exécute son programme jusqu'à une **action** (`MOVE`, `MINE`, `DROP`, `ATTACK`, `WAIT`), qui termine son tour.

## Contrôles

| Touche | Effet |
| --- | --- |
| Clic gauche | sélectionner usine / drone ; double-clic sur une usine pour éditer son programme |
| Clic droit glissé, molette | déplacer la caméra, zoomer |
| ZQSD / WASD / flèches | déplacer la caméra |
| `B` | poser une usine (Maj+clic pour en poser plusieurs) |
| `E` | éditer le programme de l'usine / déboguer le drone sélectionné |
| `P` | ouvrir l'éditeur de programmes |
| `Espace`, `1`-`4`, `N` | pause, vitesse x1 à x8, tick suivant |
| `H`, `Tab` | revenir à la base, passer d'une usine à l'autre |
| `F1` | aide complète du langage |
| Éditeur | `Ctrl+S` appliquer, `Ctrl+Z`/`Ctrl+Y`, `Ctrl+C/X/V`, `Ctrl+D` dupliquer la ligne, `Tab`/`Maj+Tab` |

« Appliquer » sauvegarde le programme dans `./programs/<nom>.dasm`, puis met à
jour toutes vos usines qui l'utilisent **et leurs drones déjà en vol**. Le
débogueur (bouton *Déboguer* sur un drone) montre ses registres, la mémoire de
ruche de son usine et surligne la dernière action exécutée ; en pause, `N`
avance tick par tick.

## DASM en bref

```asm
; MINEUR -- le programme de départ
debut:
    CARGO a
    JGE a, FULL, rentrer
    SCAN b, ORE          ; b = direction du minerai le plus proche
    JEQ b, NONE, errer
    LOOK c, b
    JEQ c, ORE, creuser
    MOVE b
    JZ ok, errer         ; bloqué : pas de côté au hasard
    JMP debut
...
```

- Registres `a b c d`, plus `ok` (lecture seule, 1 si la dernière action a réussi).
- Directions `N=0 E=1 S=2 W=3`, `NONE=-1`. Choses : `EMPTY WALL ORE ALLY ENEMY BASE EBASE`, `FULL` = capacité de la soute.
- **Données** : `SET ADD SUB MUL DIV MOD RAND INC DEC`
- **Sauts** : `JMP JZ JNZ JEQ JNE JLT JGT JLE JGE CALL RET`
- **Capteurs** (gratuits) : `SCAN DIST DX DY COUNT LOOK CARGO HP`, mémoire de ruche partagée `PEEK POKE` (16 cases par usine)
- **Actions** (finissent le tour) : `MOVE MINE DROP ATTACK WAIT`
- Au plus 32 instructions par tick ; au-delà le drone « réfléchit » et reprend au tick suivant. La fin du programme reboucle au début.

`SCAN` donne une direction naïve (l'axe dominant) et fonce dans les murs : à vous
de faire mieux. [examples/mineur_plus.dasm](examples/mineur_plus.dasm) contourne
les obstacles en essayant l'axe secondaire ; un test vérifie qu'il extrait plus
que le mineur de base. Il reste beaucoup de marge (embouteillages devant l'usine,
répartition des gisements via la mémoire de ruche, escortes...).

## Multijoueur (LAN)

Menu → *Héberger* sur une machine (port TCP 7777, l'adresse à communiquer
s'affiche), *Rejoindre* sur l'autre. Raccourcis : `--host` et `--join 192.168.1.20`.

La simulation est entièrement déterministe (entiers uniquement, RNG maison) et
tourne en **lockstep** : seules les commandes des joueurs transitent, avec 3 ticks
de délai d'entrée. Un hash de l'état est échangé tous les 32 ticks pour détecter
toute désynchronisation.

## Options de lancement

`--solo [easy|normal|hard]`, `--sandbox`, `--seed N`, `--ticks N` (avance rapide),
`--editor`, `--debug-drone`, `--help-screen`, `--build`,
`--screenshot out.png --frames N` (capture puis quitte : pratique pour tester le rendu).

## Architecture

La bibliothèque (sans graphismes, testable) :

- `src/vm.rs` : DASM, assembleur (erreurs par ligne), interpréteur, coloration syntaxique.
- `src/sim/` : monde en grille, usines, drones, économie, combat ; génération de carte symétrique.
- `src/ai.rs` : adversaire (3 niveaux) qui joue via les mêmes commandes qu'un humain.
- `src/lockstep.rs`, `src/net.rs` : lockstep à deux joueurs sur TCP (JSON par ligne).
- `src/programs.rs` : programmes fournis.

Le jeu (`src/main.rs` et voisins) : rendu néon avec bloom multi-passes (`gfx.rs`),
monde et particules (`view.rs`, `fx.rs`), interface immédiate (`ui.rs`), éditeur de
code (`editor.rs`), partie en cours (`game.rs`), menus (`menu.rs`).

```bash
cargo test
cargo test --release --lib bench_battle -- --ignored --nocapture   # perfs et équilibrage
```

## Pistes

Nouvelles actions (`BUILD` par les drones, `GIVE` entre drones), messages radio
entre usines, brouillard de guerre, types de drones, plus de 2 joueurs en ligne,
sons.
