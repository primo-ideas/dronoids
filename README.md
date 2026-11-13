# Dronoids

[![CI](https://github.com/primo-ideas/dronoids/actions/workflows/ci.yml/badge.svg)](https://github.com/primo-ideas/dronoids/actions/workflows/ci.yml)

A shared, persistent space sector, seen from above, geometric and neon.
You arrive **with nothing** but a big central **base**. It builds **dronoids**, and you
put them to work with **tasks**: "these 10 go mining until 250 crystals are in", "these
3 explore eastwards for 5 minutes". **Factories** build more of them, **beacons** let
you see further, and relics out in the sector give the base **perks**. Underneath,
every dronoid follows a **program** written in a small, playful assembly language,
**DASM**, which you can write yourself if you wish. Your colony keeps working when you
leave. An account (a nickname and a password) is needed to play
in the shared sector.

**Play: <https://primo-ideas.xyz/dronoids/>**

Inspired by *Human Resource Machine* for the language and the step-by-step debugger.

## Running it locally

```bash
cargo run --release                          # the game (desktop)
cargo run --release --bin dronoids-server    # a local sector, ws://127.0.0.1:8082/ws
./deploy/build-web.sh --serve                # the browser version, http://127.0.0.1:8090
```

A recent stable Rust (edition 2024). Dependencies: `macroquad` (rendering), `serde`,
`bincode`, `serde_json`, and `tungstenite` on the desktop and the server.

## The sector

- A **huge** space (a million units across, eleven days of travel in a straight line
  for a dronoid), generated as it is discovered: asteroid belts, empty or crowded
  regions, crystal fields that **grow back** a few hours after they run out, and AI
  colonies living their own lives.
- **Arrival**: a base and 40 crystals, nothing else. You choose where your base
  lands (away from the other colonies); the landing is free. A **shield** protects
  newcomers for 10 minutes from the moment they land: they can neither be attacked nor
  attack.
- **The base**: one per colony, large (6 units across), 3000 points of hull, mending
  itself slowly (a point a second). When it falls, the colony is gone with all its
  dronoids, and the player lands anew.
- **Production lines**: a base starts with two, each with its **own quota**, building
  in parallel: a dronoid every 30 s (the first one of a line comes quickly), for 10
  crystals. A line can be paused. New dronoids wait at home for a task.
- **Tasks**: a kind (**Mine**, **Explore**, **Prospect** for relics, **Guard**, **Raid**,
  or **Custom**: a program of your own), how many dronoids, until when (so many
  crystals or shards brought home, so many minutes, or until stopped) and a few
  options (Explore: a heading; Raid: a target). A task takes that many dronoids
  waiting at home, and more as they come; once its objective is met, they come home
  and wait again. A task can be stopped, launched again, or set to start over by
  itself. A new base lands with a first one, mining with 4 dronoids until stopped.
- **Factories** (150 crystals), built where the base or a beacon sees: each builds
  dronoids of its own, makes room for 8 more, and is a place to unload. 600 points
  of hull. They do not see.
- **Beacons** (50 crystals), built anywhere you see right now (around your base, a
  beacon or any of your dronoids): each sees 30 units around it, and maps it at once.
- **Room**: a colony holds at most 12 dronoids, plus 8 a factory and what the Hangar
  perk adds, up to 150.
- **First steps**: seven objectives, one after the other (land, mine 100 crystals,
  launch a task, map the surroundings, build a factory, place a beacon, bring home a
  relic shard), each with a small reward in crystals. They are part of the world:
  progress follows the player from one session and one device to the next.
- **Fog of war**: you only see around your base (24 units, more with Vision),
  your beacons (30) and your dronoids (12). What the base and the beacons see is
  outlined on the map. Space explored already stays in memory, veiled (asteroids and crystals, but no
  foreign unit); the unknown stays black. The explored map belongs to each player and
  persists: your dronoids keep mapping while you are away. (Every client replays the
  whole world: the fog is a choice of display, not a protection.)
- **A slow pace**: 4 ticks a second; a step takes 1 s, mining a crystal 2 s, and a
  production line turns out a dronoid every 30 s.
- The AIs only raid around their own home, and keep idle hunters there (the hardest
  ones let them roam): settling far from them means growing in
  peace.
- **Perks, from relics**: rare remains (about one square of 64 units in five, never
  within 50 units of a starting place), each of 12 shards, of ten kinds. A dronoid
  that `MINE`s one sends the shard to its base; every 4 shards of a kind raise the
  base one level of that perk (a whole relic is three levels), up to **100 levels**:

  | Perk | A level | At level 100 |
  | --- | --- | --- |
  | **Forge** | every line builds 3 % faster | 4 times as fast |
  | **Assembly** | a production line every 20 levels, running what the first one runs | 7 lines |
  | **Refinery** | 2 % more of every unload (fractions add up) | 3 times as much |
  | **Bulwark** | 50 more hull | 8000 hull |
  | **Arsenal** | a tenth of a point more per shot (the extra point comes that often) | 12 a shot |
  | **Vision** | the base sees 1 unit further (and maps it at once) | 124 units |
  | **Drills** | its dronoids mine 2 % faster | 3 times as fast |
  | **Armor** | a point more hull for new dronoids every 5 levels | 30 hull |
  | **Hangar** | room for 1 more dronoid | 100 more |
  | **Nanites** | the base mends a point more a second every 5 levels | 21 a second |

  A base's perks go with it when it falls; a relic mined out is whole again a day
  later. The built-in `prospector` looks for them, naively; the miners ignore them.
- **Accounts**: the shared sector only lets in players signed in on the site, with a
  nickname and a password. A sector of one's own (a server started without the site)
  lets anyone in, the nickname then protected by a secret token kept by the browser
  (or `./save/` on the desktop).
- **Offline training**: a private sector with AIs, pause and speed up to x8.

## Controls

| Key | Effect |
| --- | --- |
| Left click | land the base; select; build; double-click the base for the tasks |
| Right drag, wheel | move the camera, zoom (out to a view of the whole explored area) |
| ZQSD / WASD / arrows | move the camera |
| `K` | the tasks |
| `F` / `B` | build a factory / a beacon (Shift+click places several) |
| `E` / `P` | the selected dronoid's debugger / the programs |
| `M`, `L`, `H` or `Tab` | map, colonies, back to the base |
| `F1` | language help |
| Offline | `Space` pause, `1`-`4` speed, `N` next tick |

On a phone or a tablet:

| Gesture | Effect |
| --- | --- |
| Tap | select, land the base, build, place a raid's target; twice on the base for the tasks |
| One finger sliding | move the camera (in the editor and the help: scroll) |
| Two fingers | zoom, and move the camera |
| Tap a text field or the code | bring up the phone's keyboard |

The screen stays clean on purpose: your crystals at the top left (and the newcomer's
shield while it lasts), speed, help and menu at the top right, a dock at the bottom,
and one short line: the current tool, or else the next first step, how far along it
is, how to go about it and what it earns. Beside the crystals, the dronoids out of
the hangar's room; the base's panel has one row per production line (its program,
its dronoids out of its quota, its code, a pause; pointing at it says when its next
dronoid comes, or why none does), Expand, the target, and what relics gave the base;
pointing at a relic says what it is; a warning tells of a raid on the base, which may
be off screen. A unit's panel only shows its health
once it has been hit; map and colonies open
on demand. The editor, the language help and the dialogs are opaque, and the last two
dim the game behind them (a click beside the help puts it away); with the editor
open, the camera keeps the base or the dronoid being followed in the part of the
map left free beside it. The one button a screen is there for (enter the sector,
sign in, apply a program) is outlined and brighter than the others. Offline, the speed button goes from x1 to x2, x4, x8 then pause. On a
narrow screen (under 700 px wide, a phone held upright), the selection panel moves to
the bottom above the dock, and the editor and the help take the whole screen.

## DASM in short

A program is a column of **tiles**, in the spirit of Human Resource Machine: drag
instructions in from the palette on the right, drag them around to reorder them, drag
one out to the palette to delete it, touch a token (`ORE`, `> home`) to change it.
Each jump is drawn as an arrow to its label. "Text" shows the same program as DASM
source, to type it; both are the same program.

```asm
; MINER -- the starting program
start:
    IFFULL home
    IFNEAR ORE, dig
    GO ORE
    IFFAIL wander        ; nothing in sight, or blocked
    JUMP start
dig:
    MINE
    JUMP start
home:
    IFNEAR BASE, unload
    GO BASE
...
```

- **The simple instructions** cover most programs: `GO thing` (a step towards the
  nearest one), `EXPLORE` (a step straight on, a new course when blocked), `MINE`,
  `DROP`, `ATTACK`, `WAIT`, `JUMP lbl`, and the jumps `IFFULL lbl`, `IFNEAR thing, lbl`
  (within reach: to mine, unload or fire at), `IFSEE thing, lbl` and `IFFAIL lbl`
  (the last action failed).
  Things: `ORE BASE ENEMY EBASE RELIC ALLY WALL EMPTY`.
- **More** (the palette's button) gives the rest: registers `a b c d`, plus `ok` (1 if
  the last action succeeded); angles in degrees, clockwise (`N=0 E=90 S=180 W=270`,
  `NONE=-1`); **Data** `SET ADD SUB MUL DIV MOD RAND INC DEC ANGLE` · **Jumps** `JMP JZ
  JNZ JEQ JNE JLT JGT JLE JGE CALL RET` · **Sensors** `SCAN DIST DX DY COUNT LOOK CARGO
  HP POSX POSY PEEK POKE` · **Actions** `MOVE angle`. `FULL` = the hold's capacity.
- `MINE`/`DROP` when `DIST ≤ 1`, `ATTACK` when `DIST ≤ 2`. `MINE` takes a shard of a
  relic within reach when no crystal is nearer. Sensors reach 12 units,
  enemy bases 40, your own anywhere. 16 cells of hive memory, shared by all the
  base's dronoids, whatever their line.

[examples/miner_plus.dasm](examples/miner_plus.dasm) looks ahead (`LOOK`) and sets off
exploring in a straight line: a test checks that over time it mines clearly more than
the basic miner (+20 % after half an hour).

The built-in programs were French until November 2026 (`mineur`, `eclaireur`,
`chasseur`, `gardien`): copies kept by players and by a saved sector take their
English names (`miner`, `scout`, `hunter`, `guard`) when they are loaded, and untouched
copies their English wording, whose instructions are the same line for line.
In November 2026 the built-ins were rewritten with the simple instructions: an untouched
copy of an earlier wording in a player's library takes the new one (the dronoids already
running keep their program until it is applied again).

## How multiplayer works

The server (`dronoids-server`) owns the reference world and runs the AIs. Every tick,
it broadcasts the ordered list of the players' commands; each client replays those
commands on its own copy. The simulation is fully deterministic (fixed-point integers,
a sine table, integer `atan2` and square roots, a home-made RNG), so only commands
travel, never the thousands of dronoids. Asteroids and crystals are never sent: every
machine computes them from the seed and the place looked at, and only deposits that
have been mined are kept. A newcomer gets a snapshot of the world, then the stream of
commands. Clients send a regular hash; a client that diverges, or has fallen too far
behind (a tab in the background), gets a new snapshot.

Everything goes through WebSocket (bincode messages): `tungstenite` on the server and
the desktop, and in the browser the page's WebSocket, through a small miniquad plugin
([web/god_net.js](web/god_net.js)) that also provides local storage.

## The site

The game lives in a small site (`site/`): an introduction, and the accounts, made of a
nickname and a password, which keep the colony on every device. An account is needed
to play in the shared sector. Signing in with Google, Discord, GitHub or Twitch is
written and will be offered later. See `docs/deploy.md`.

## Going online

See [docs/deploy.md](docs/deploy.md). In short: `./deploy/deploy.sh` builds the server
for Linux (static musl, linked with `rust-lld`) and the WASM client, uploads it all and
installs it on the machine.

## Architecture

The library (no graphics, testable):

- `src/vm.rs`: DASM, assembler, interpreter, syntax highlighting.
- `src/geo.rs`: deterministic integer geometry.
- `src/sim/`: the world (landing, the bases and their production lines, perks,
  shield, crystals growing back, each player's explored map), the map, the spatial
  index, the first steps (`quests.rs`) and reading the saves of earlier versions
  (`compat.rs`, which also turns the factories of saves up to protocol 9 into bases).
- `src/ai.rs`: the AI colonies, which play through the same commands as a human.
- `src/server.rs`, `src/net.rs`: the world server, the WebSocket protocol.
- `src/bin/server.rs`: `dronoids-server`.

The game: neon rendering with bloom (`gfx.rs`), world and particles (`view.rs`,
`fx.rs`), interface (`ui.rs`, `game.rs`, `menu.rs`), editor (`editor.rs`),
desktop/browser differences (`platform.rs`, `library.rs`).

```bash
cargo test
python3 site/test_site.py                                           # the account service
cargo test --release --lib bench_world -- --ignored --nocapture   # performance and AI pace
cargo test --release --lib balance_report -- --ignored --nocapture   # economy, scouts, raids, wars
```

Every push to `main` and every pull request goes through the same on GitHub
(`.github/workflows/ci.yml`): clippy with warnings refused and the tests, on Linux and
Windows, the account service's tests, and the two builds that get deployed (the browser
client and the static Linux server), kept as artifacts of the run.

Font: Fira Mono (SIL Open Font License, see `assets/fonts`).
