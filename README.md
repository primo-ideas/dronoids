# Dronoids

[![CI](https://github.com/primo-ideas/dronoids/actions/workflows/ci.yml/badge.svg)](https://github.com/primo-ideas/dronoids/actions/workflows/ci.yml)

A shared, persistent space sector, seen from above, geometric and neon.
You arrive **with nothing** but a big central **base**. Its production lines build
**dronoids**, which follow to the letter a **program** written in a small, playful
assembly language, **DASM**. The programs provided are naive on purpose: the game is
writing better ones. Relics out in the sector give the base **perks**. Your colony
keeps working when you leave. An account (a nickname and a password) is needed to play
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
- **Production lines**: a base starts with two. Each line has its **own program**
  and its **own quota**, and builds in parallel with the others: a dronoid every 30 s
  (the first one of a line comes quickly), for 10 crystals. A line can be paused.
  Giving a line another program reprograms its dronoids too.
- **The hangar**: all lines together hold at most the hangar's room, 12 dronoids at
  first. **Expand** (150 crystals) makes room for 6 more, up to 150.
- **First steps**: six objectives, one after the other (land, mine 100 crystals,
  apply a program of your own, map the surroundings, enlarge the hangar, bring home a
  relic shard), each with a small reward in crystals. They are part of the world:
  progress follows the player from one session and one device to the next.
- **Fog of war**: you only see around your base (24 units) and your dronoids
  (12). Space explored already stays in memory, veiled (asteroids and crystals, but no
  foreign unit); the unknown stays black. The explored map belongs to each player and
  persists: your dronoids keep mapping while you are away. (Every client replays the
  whole world: the fog is a choice of display, not a protection.)
- **A slow pace**: 4 ticks a second; a step takes 1 s, mining a crystal 2 s, and a
  production line turns out a dronoid every 30 s.
- The AIs only raid around their own home, and keep idle hunters there (the hardest
  ones let them roam): settling far from them means growing in
  peace.
- **Perks, from relics**: rare remains (about one square of 64 units in five, never
  within 50 units of a starting place), each of 12 shards, of five kinds. A dronoid
  that `MINE`s one sends the shard to its base; 12 shards of a kind raise the base one
  level of that perk, up to 3: **Forge** (every line waits 20 % less a level between
  two dronoids), **Assembly** (one more production line a level, running what the
  first one runs: up to five lines), **Refinery** (one more crystal a level for every
  5 unloaded), **Bulwark** (1000 more hull a level), **Arsenal** (its dronoids' shots
  do 1 more damage a level). A base's perks go with it when it falls; a relic mined
  out is whole again a day later. The built-in `prospector` looks for them, naively;
  the miners ignore them.
- **Accounts**: the shared sector only lets in players signed in on the site, with a
  nickname and a password. A sector of one's own (a server started without the site)
  lets anyone in, the nickname then protected by a secret token kept by the browser
  (or `./save/` on the desktop).
- **Offline training**: a private sector with AIs, pause and speed up to x8.

## Controls

| Key | Effect |
| --- | --- |
| Left click | land the base; select; double-click the base for its first line's program |
| Right drag, wheel | move the camera, zoom |
| ZQSD / WASD / arrows | move the camera |
| `X` | expand the hangar (150 crystals) |
| `E` / `P` | the first line's program, the dronoid's debugger / all programs |
| `T` | set the base's target (read by `hunter`) |
| `M`, `L`, `H` or `Tab` | map, colonies, back to the base |
| `F1` | language help |
| Offline | `Space` pause, `1`-`4` speed, `N` next tick |

On a phone or a tablet:

| Gesture | Effect |
| --- | --- |
| Tap | select, land the base, place a target; twice on the base for its first line's program |
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

```asm
; MINER -- the starting program
start:
    CARGO a
    JGE a, FULL, home
    DIST a, ORE          ; distance to the nearest crystal
    JEQ a, NONE, wander  ; nothing in sight?
    JLE a, 1, dig        ; within reach: dig
    SCAN b, ORE          ; b = angle to the crystal
    MOVE b
    JZ ok, dodge         ; blocked by an asteroid
    JMP start
...
```

- Registers `a b c d`, plus `ok` (1 if the last action succeeded).
- Angles in degrees, clockwise: `N=0 E=90 S=180 W=270`, `NONE=-1`.
  Things: `EMPTY WALL ORE ALLY ENEMY BASE EBASE RELIC`, `FULL` = the hold's capacity.
- **Data** `SET ADD SUB MUL DIV MOD RAND INC DEC ANGLE` · **Jumps** `JMP JZ JNZ
  JEQ JNE JLT JGT JLE JGE CALL RET` · **Sensors** `SCAN DIST DX DY COUNT LOOK CARGO
  HP POSX POSY PEEK POKE` · **Actions** `MOVE angle`, `MINE`, `DROP`, `ATTACK`, `WAIT`.
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
