# Dronoids

A shared, persistent space sector, seen from above, geometric and neon.
You arrive **with nothing** but what it takes to build a factory. The factory builds
**dronoids**, which follow to the letter a **program** written in a small, playful
assembly language, **DASM**. The programs provided are naive on purpose: the game is
writing better ones. Your colony keeps working when you leave.

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
- **Arrival**: 170 crystals, nothing else. You choose where to place your first factory
  (away from the other colonies). A **shield** protects newcomers for 10 minutes from
  the moment they land: they can neither be attacked nor attack.
  A new factory turns out its first two dronoids quickly.
- **First steps**: five objectives, one after the other (land, mine 100 crystals,
  apply a program of your own, map the surroundings, found a second factory), each
  with a small reward in crystals. They are part of the world: progress follows the
  player from one session and one device to the next.
- **Fog of war**: you only see around your factories (18 units) and your dronoids
  (12). Space explored already stays in memory, veiled (asteroids and crystals, but no
  foreign unit); the unknown stays black. The explored map belongs to each player and
  persists: your dronoids keep mapping while you are away. (Every client replays the
  whole world: the fog is a choice of display, not a protection.)
- **A slow pace**: 4 ticks a second; a step takes 1 s, mining a crystal 2 s, and a
  factory then turns out a dronoid every 30 s.
- The AIs only raid around their own home: settling far from them means growing in
  peace.
- Your nickname is protected by a secret token kept by your browser (or `./save/` on
  the desktop).
- **Offline training**: a private sector with AIs, pause and speed up to x8.

## Controls

| Key | Effect |
| --- | --- |
| Left click | select; double-click a factory for its program |
| Right drag, wheel | move the camera, zoom |
| ZQSD / WASD / arrows | move the camera |
| `B` | new factory (Shift+click to place several) |
| `E` / `P` | the factory's program, the dronoid's debugger / all programs |
| `T` | set the factory's target (read by `hunter`) |
| `M`, `L`, `H`, `Tab` | map, colonies, back to base, next factory |
| `F1` | language help |
| Offline | `Space` pause, `1`-`4` speed, `N` next tick |

On a phone or a tablet:

| Gesture | Effect |
| --- | --- |
| Tap | select, place a factory or a target; twice on a factory for its program |
| One finger sliding | move the camera (in the editor and the help: scroll) |
| Two fingers | zoom, and move the camera |
| Tap a text field or the code | bring up the phone's keyboard |

The screen stays clean on purpose: your crystals at the top left (and the newcomer's
shield while it lasts), speed, help and menu at the top right, a dock at the bottom,
and one short line: the current tool, or else the next first step and how far along it
is. A unit's panel only shows its health once it has been hit; map and colonies open
on demand. Offline, the speed button goes from x1 to x2, x4, x8 then pause. On a
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
  Things: `EMPTY WALL ORE ALLY ENEMY BASE EBASE`, `FULL` = the hold's capacity.
- **Data** `SET ADD SUB MUL DIV MOD RAND INC DEC ANGLE` · **Jumps** `JMP JZ JNZ
  JEQ JNE JLT JGT JLE JGE CALL RET` · **Sensors** `SCAN DIST DX DY COUNT LOOK CARGO
  HP POSX POSY PEEK POKE` · **Actions** `MOVE angle`, `MINE`, `DROP`, `ATTACK`, `WAIT`.
- `MINE`/`DROP` when `DIST ≤ 1`, `ATTACK` when `DIST ≤ 2`. Sensors reach 12 units,
  enemy factories 40, your own anywhere. 16 cells of hive memory per factory.

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

The game lives in a small site (`site/`): an introduction, and accounts made with
Google, Discord, GitHub or Twitch, which keep the nickname and the colony on every
device. Playing needs no account. See `docs/deploy.md`.

## Going online

See [docs/deploy.md](docs/deploy.md). In short: `./deploy/deploy.sh` builds the server
for Linux (static musl, linked with `rust-lld`) and the WASM client, uploads it all and
installs it on the machine.

## Architecture

The library (no graphics, testable):

- `src/vm.rs`: DASM, assembler, interpreter, syntax highlighting.
- `src/geo.rs`: deterministic integer geometry.
- `src/sim/`: the world (landing, shield, crystals growing back, each player's
  explored map), the map, the spatial index, the first steps (`quests.rs`) and
  reading the saves of earlier versions (`compat.rs`).
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
```

Font: Fira Mono (SIL Open Font License, see `assets/fonts`).
