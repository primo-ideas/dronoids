//! Game of Drones — program drone swarms in a tiny assembly language, in a
//! large world shared with computer players or other humans.

mod editor;
mod fx;
mod game;
mod gfx;
mod library;
mod menu;
mod ui;
mod view;

use macroquad::prelude::*;

use game::{Game, Outcome};
use game_of_drones::ai::Level;
use gfx::Bloom;
use library::Library;
use menu::{Menu, MenuOut};
use ui::Ui;

enum App {
    Menu(Box<Menu>),
    Game(Box<Game>),
}

fn window_conf() -> Conf {
    Conf {
        window_title: "Game of Drones".to_owned(),
        window_width: 1600,
        window_height: 900,
        window_resizable: true,
        sample_count: 4,
        ..Default::default()
    }
}

/// Command-line switches, mostly for quick testing:
/// `--solo [easy|normal|hard]`, `--ais N`, `--sandbox`, `--seed N`,
/// `--ticks N` (fast-forward), `--zoom Z`, `--editor`, `--debug-drone`,
/// `--help-screen`, `--build`, `--screenshot out.png [--frames N | --after SECS]`,
/// `--host WORLD`, `--join ADDR`, `--name PSEUDO`.
#[derive(Default)]
struct Args {
    solo: Option<Level>,
    ais: Option<usize>,
    sandbox: bool,
    seed: Option<u64>,
    screenshot: Option<String>,
    frames: u32,
    after: Option<f64>,
    editor: bool,
    debug_drone: bool,
    help: bool,
    build: bool,
    ticks: u64,
    zoom: Option<f32>,
    host: Option<String>,
    join: Option<String>,
    name: Option<String>,
    worlds: bool,
}

fn parse_args() -> Args {
    let mut a = Args { frames: 60, ..Default::default() };
    let v: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < v.len() {
        let next = v.get(i + 1).cloned().unwrap_or_default();
        let mut take = true;
        match v[i].as_str() {
            "--solo" => {
                let lvl = match next.as_str() {
                    "easy" => Some(Level::Easy),
                    "hard" => Some(Level::Hard),
                    "normal" => Some(Level::Normal),
                    _ => None,
                };
                take = lvl.is_some();
                a.solo = Some(lvl.unwrap_or(Level::Normal));
            }
            "--ais" => a.ais = next.parse().ok(),
            "--sandbox" => {
                a.sandbox = true;
                take = false;
            }
            "--seed" => a.seed = next.parse().ok(),
            "--screenshot" => a.screenshot = Some(next),
            "--frames" => a.frames = next.parse().unwrap_or(60),
            "--after" => a.after = next.parse().ok(),
            "--ticks" => a.ticks = next.parse().unwrap_or(0),
            "--zoom" => a.zoom = next.parse().ok(),
            "--host" => a.host = Some(next),
            "--join" => a.join = Some(next),
            "--name" => a.name = Some(next),
            flag => {
                take = false;
                match flag {
                    "--editor" => a.editor = true,
                    "--debug-drone" => a.debug_drone = true,
                    "--help-screen" => a.help = true,
                    "--build" => a.build = true,
                    "--worlds" => a.worlds = true,
                    _ => {}
                }
            }
        }
        i += if take { 2 } else { 1 };
    }
    a
}

#[macroquad::main(window_conf)]
async fn main() {
    let args = parse_args();
    gfx::init_font();
    let mut bloom = Bloom::new();
    let mut lib = Library::load();
    let mut ui = Ui::default();

    let mut app = if args.solo.is_some() || args.sandbox {
        let level = args.solo.unwrap_or(Level::Normal);
        let ais = if args.sandbox { 0 } else { args.ais.unwrap_or(5) };
        let mut g = Game::solo(&lib, args.seed.unwrap_or_else(game::new_seed), ais, level);
        g.fast_forward(args.ticks);
        if let Some(z) = args.zoom {
            g.set_zoom(z);
        }
        if args.editor {
            g.open_program_by_name(&lib, "mineur");
        }
        if args.debug_drone {
            g.debug_first_drone();
        }
        g.set_overlays(args.help, args.build);
        App::Game(Box::new(g))
    } else {
        let mut m = Menu::new();
        if let Some(n) = &args.name {
            m.set_player_name(n);
        }
        if args.worlds {
            m.show_worlds();
        }
        if let Some(w) = &args.host {
            m.host(w);
        } else if let Some(addr) = &args.join {
            m.connect(addr, None);
        }
        App::Menu(Box::new(m))
    };

    let mut frame = 0u32;
    loop {
        ui.begin_frame();
        let mut next = None;
        match &mut app {
            App::Menu(m) => match m.frame(&mut ui, &mut bloom) {
                MenuOut::Stay => {}
                MenuOut::Quit => break,
                MenuOut::Solo { ais, level } => {
                    next = Some(App::Game(Box::new(Game::solo(&lib, game::new_seed(), ais, level))));
                }
                MenuOut::Online { conn, me, world, name, server } => {
                    let mut g = Game::online(&lib, conn, me, *world, name, server);
                    if let Some(z) = args.zoom {
                        g.set_zoom(z);
                    }
                    next = Some(App::Game(Box::new(g)));
                }
            },
            App::Game(g) => {
                if let Outcome::ToMenu = g.frame(&mut ui, &mut lib, &mut bloom) {
                    next = Some(App::Menu(Box::new(Menu::new())));
                }
            }
        }
        if let Some(n) = next {
            app = n;
        }

        frame += 1;
        let due = match args.after {
            Some(secs) => get_time() >= secs,
            None => frame >= args.frames,
        };
        if let Some(path) = &args.screenshot
            && due
        {
            let mut img = get_screen_data();
            for px in img.bytes.chunks_mut(4) {
                px[3] = 255;
            }
            img.export_png(path);
            break;
        }
        next_frame().await;
    }
    if let App::Game(g) = &mut app {
        g.leave();
    }
}
