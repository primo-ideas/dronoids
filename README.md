# Dronoids

Un secteur spatial partagé et persistant, vue du dessus, géométrique et néon.
On y arrive **sans rien**, sinon de quoi bâtir une usine. L'usine fabrique des
**dronoids**, qui suivent à la lettre un **programme** écrit dans un petit assembleur
ludique, **DASM**. Les programmes fournis sont volontairement naïfs : le jeu, c'est
d'en écrire de meilleurs. Votre colonie continue de travailler quand vous partez.

**Jouer : <https://primo-ideas.xyz/dronoids/>**

Inspiré de *Human Resource Machine* pour le langage et le débogueur pas à pas.

## Lancer en local

```bash
cargo run --release                          # le jeu (bureau)
cargo run --release --bin dronoids-server    # un secteur local, ws://127.0.0.1:8082/ws
./deploy/build-web.sh --serve                # la version navigateur, http://127.0.0.1:8090
```

Rust stable récent (édition 2024). Dépendances : `macroquad` (rendu), `serde`,
`bincode`, `serde_json`, et `tungstenite` côté bureau/serveur.

## Le secteur

- Un espace **immense** (un million d'unités de large, onze jours de voyage en ligne
  droite pour un dronoid), généré à mesure qu'on le découvre : ceintures d'astéroïdes,
  régions désertes ou encombrées, champs de cristaux qui **repoussent** quelques
  heures après avoir été épuisés, et des colonies IA qui vivent leur vie.
- **Arrivée** : 170 cristaux, rien d'autre. On choisit où poser sa première usine
  (loin des autres colonies). Un **bouclier** protège les nouveaux venus 10 minutes
  à partir du moment où ils se posent : ils ne peuvent ni être attaqués, ni attaquer.
  Une usine neuve sort ses deux premiers dronoids vite.
- **Premiers pas** : cinq objectifs, l'un après l'autre (se poser, extraire 100
  cristaux, appliquer un programme de son cru, cartographier les environs, fonder une
  deuxième usine), chacun avec sa petite récompense en cristaux. Ils font partie du
  monde : la progression suit le joueur d'une session et d'un appareil à l'autre.
- **Brouillard de guerre** : on ne voit qu'autour de ses usines (18 unités) et de
  ses dronoids (12). L'espace déjà exploré reste en mémoire, voilé (astéroïdes et
  cristaux, mais aucune unité étrangère) ; l'inconnu reste noir. La carte explorée
  est propre à chaque joueur et persiste : vos dronoids cartographient aussi pendant
  votre absence. (Chaque client rejoue tout le monde : le brouillard est un choix
  d'affichage, pas une protection.)
- **Rythme lent** : 4 ticks par seconde ; un pas prend 1 s, extraire un cristal 2 s,
  une usine produit ensuite un dronoid toutes les 30 s.
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
| `E` / `P` | programme de l'usine, débogueur du dronoid / tous les programmes |
| `T` | poser la cible de l'usine (lue par `chasseur`) |
| `M`, `L`, `H`, `Tab` | carte, colonies, retour à la base, usine suivante |
| `F1` | aide du langage |
| Hors ligne | `Espace` pause, `1`-`4` vitesse, `N` tick suivant |

Sur téléphone ou tablette :

| Geste | Effet |
| --- | --- |
| Toucher | sélectionner, poser une usine ou une cible ; deux fois sur une usine pour son programme |
| Un doigt glissé | déplacer la caméra (dans l'éditeur et l'aide : faire défiler) |
| Deux doigts | zoomer, et déplacer la caméra |
| Toucher un champ de texte ou le code | ouvrir le clavier du téléphone |

L'écran reste volontairement épuré : vos cristaux en haut à gauche (et la
protection du nouveau venu tant qu'elle dure), vitesse, aide et menu en haut à
droite, un dock en bas, et une ligne courte : l'outil en cours, sinon le prochain
premier pas et son avancement. Le panneau d'une unité ne montre sa santé qu'une
fois touchée ; carte et colonies s'ouvrent à la demande. Hors ligne, le bouton de
vitesse passe de x1 à x2, x4, x8 puis pause. Sur un écran étroit (moins de 700 px de
large, un téléphone tenu droit), le panneau de sélection passe en bas au-dessus du
dock, et l'éditeur comme l'aide occupent tout l'écran.

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
dronoids. Astéroïdes et cristaux ne sont jamais envoyés : chaque machine les calcule
à partir de la graine et de l'endroit regardé, et seuls les gisements entamés sont
gardés. Un arrivant reçoit un instantané du monde, puis le flux de commandes. Les clients envoient un hash
régulier ; un client qui diverge, ou qui a pris trop de retard (onglet en
arrière-plan), reçoit un nouvel instantané.

Tout passe par WebSocket (messages bincode) : `tungstenite` côté serveur et bureau,
et dans le navigateur la WebSocket de la page, via un petit plugin miniquad
([web/god_net.js](web/god_net.js)) qui fournit aussi le stockage local.

## Le site

Le jeu vit dans un petit site (`site/`) : présentation, et comptes créés avec
Google, Discord, GitHub ou Twitch, qui gardent le pseudo et la colonie sur tous les
appareils. On peut toujours jouer sans compte. Voir `docs/deploy.md`.

## Mise en ligne

Voir [docs/deploy.md](docs/deploy.md). En bref : `./deploy/deploy.sh` compile le
serveur pour Linux (musl statique, lié par `rust-lld`) et le client WASM, envoie le
tout et l'installe sur la machine.

## Architecture

La bibliothèque (sans graphismes, testable) :

- `src/vm.rs` : DASM, assembleur, interpréteur, coloration syntaxique.
- `src/geo.rs` : géométrie entière déterministe.
- `src/sim/` : le monde (atterrissage, bouclier, repousse des cristaux, carte
  explorée de chaque joueur), la carte, l'index spatial, les premiers pas
  (`quests.rs`) et la lecture des sauvegardes d'anciennes versions (`compat.rs`).
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
