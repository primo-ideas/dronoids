//! Game of Drones — program drone swarms in a tiny assembly language.

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
/// `--solo [easy|normal|hard]`, `--sandbox`, `--seed N`,
/// `--screenshot out.png --frames N`, `--editor`, `--debug-drone`,
/// `--ticks N` (fast-forward the simulation before showing it),
/// `--host`, `--join ADDR` (LAN game without going through the menu).
struct Args {
    solo: Option<Option<game_of_drones::ai::Level>>,
    seed: Option<u64>,
    screenshot: Option<String>,
    frames: u32,
    editor: bool,
    debug_drone: bool,
    help: bool,
    build: bool,
    ticks: u64,
    host: bool,
    join: Option<String>,
}

fn parse_args() -> Args {
    use game_of_drones::ai::Level;
    let mut a = Args { solo: None, seed: None, screenshot: None, frames: 60, editor: false, debug_drone: false, help: false, build: false, ticks: 0, host: false, join: None };
    let v: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < v.len() {
        let next = v.get(i + 1).cloned().unwrap_or_default();
        match v[i].as_str() {
            "--solo" => {
                let lvl = match next.as_str() {
                    "easy" => Some(Level::Easy),
                    "hard" => Some(Level::Hard),
                    "normal" => Some(Level::Normal),
                    _ => None,
                };
                if lvl.is_some() {
                    i += 1;
                }
                a.solo = Some(Some(lvl.unwrap_or(Level::Normal)));
            }
            "--sandbox" => a.solo = Some(None),
            "--seed" => {
                a.seed = next.parse().ok();
                i += 1;
            }
            "--screenshot" => {
                a.screenshot = Some(next);
                i += 1;
            }
            "--frames" => {
                a.frames = next.parse().unwrap_or(60);
                i += 1;
            }
            "--ticks" => {
                a.ticks = next.parse().unwrap_or(0);
                i += 1;
            }
            "--editor" => a.editor = true,
            "--debug-drone" => a.debug_drone = true,
            "--help-screen" => a.help = true,
            "--build" => a.build = true,
            "--host" => a.host = true,
            "--join" => {
                a.join = Some(next);
                i += 1;
            }
            _ => {}
        }
        i += 1;
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

    let mut app = match args.solo {
        Some(level) => {
            let mut g = Game::solo(&lib, args.seed.unwrap_or_else(game::new_seed), level);
            g.fast_forward(args.ticks);
            if args.editor {
                g.open_program_by_name(&lib, "mineur");
            }
            if args.debug_drone {
                g.debug_first_drone();
            }
            g.set_overlays(args.help, args.build);
            App::Game(Box::new(g))
        }
        None => {
            let mut m = Menu::new();
            if args.host {
                m.host();
            } else if let Some(addr) = &args.join {
                m.join(addr);
            }
            App::Menu(Box::new(m))
        }
    };

    let mut frame = 0u32;
    loop {
        ui.begin_frame();
        let mut next = None;
        match &mut app {
            App::Menu(m) => match m.frame(&mut ui, &mut bloom) {
                MenuOut::Stay => {}
                MenuOut::Quit => break,
                MenuOut::Solo(level) => {
                    next = Some(App::Game(Box::new(Game::solo(&lib, game::new_seed(), level))));
                }
                MenuOut::Net { peer, seed, names, me } => {
                    next = Some(App::Game(Box::new(Game::net(&lib, seed, names, me, peer))));
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
        if let Some(path) = &args.screenshot
            && frame >= args.frames {
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
