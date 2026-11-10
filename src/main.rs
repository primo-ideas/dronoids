//! Dronoids — program dronoid colonies in a tiny assembly language, in
//! a shared, persistent sector (or offline, for practice).

mod editor;
mod fx;
mod game;
mod gfx;
mod library;
mod menu;
mod platform;
mod ui;
mod view;

use macroquad::prelude::*;

use game::{Game, Outcome};
use dronoids::ai::Level;
use gfx::Bloom;
use library::Library;
use menu::{Menu, MenuOut};
use platform::Settings;
use ui::Ui;

/// Checked by `web/god_net.js`.
#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn god_crate_version() -> u32 {
    1
}

enum App {
    Menu(Box<Menu>),
    Game(Box<Game>),
}

fn window_conf() -> Conf {
    Conf {
        window_title: "Dronoids".to_owned(),
        window_width: 1600,
        window_height: 900,
        window_resizable: true,
        high_dpi: false,
        sample_count: 4,
        ..Default::default()
    }
}

/// Command-line switches, mostly for quick testing:
/// `--offline [easy|normal|hard]`, `--ais N`, `--seed N`, `--ticks N`
/// (fast-forward), `--land` (place the first factory), `--zoom Z`,
/// `--editor`, `--debug-dronoid`, `--help-screen`, `--build`, `--map`,
/// `--players`, `--join URL`, `--name NICKNAME`,
/// `--screenshot out.png [--after SECS | --frames N]`.
#[derive(Default)]
struct Args {
    offline: Option<Level>,
    ais: Option<usize>,
    seed: Option<u64>,
    screenshot: Option<String>,
    frames: u32,
    after: Option<f64>,
    editor: bool,
    debug_dronoid: bool,
    help: bool,
    build: bool,
    map: bool,
    players: bool,
    land: bool,
    ticks: u64,
    zoom: Option<f32>,
    join: Option<String>,
    name: Option<String>,
}

fn parse_args() -> Args {
    let mut a = Args { frames: 60, ..Default::default() };
    let v: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < v.len() {
        let next = v.get(i + 1).cloned().unwrap_or_default();
        let mut take = true;
        match v[i].as_str() {
            "--offline" => {
                let lvl = match next.as_str() {
                    "easy" => Some(Level::Easy),
                    "hard" => Some(Level::Hard),
                    "normal" => Some(Level::Normal),
                    _ => None,
                };
                take = lvl.is_some();
                a.offline = Some(lvl.unwrap_or(Level::Normal));
            }
            "--ais" => a.ais = next.parse().ok(),
            "--seed" => a.seed = next.parse().ok(),
            "--screenshot" => a.screenshot = Some(next),
            "--frames" => a.frames = next.parse().unwrap_or(60),
            "--after" => a.after = next.parse().ok(),
            "--ticks" => a.ticks = next.parse().unwrap_or(0),
            "--zoom" => a.zoom = next.parse().ok(),
            "--join" => a.join = Some(next),
            "--name" => a.name = Some(next),
            flag => {
                take = false;
                match flag {
                    "--editor" => a.editor = true,
                    "--debug-dronoid" => a.debug_dronoid = true,
                    "--help-screen" => a.help = true,
                    "--build" => a.build = true,
                    "--map" => a.map = true,
                    "--players" => a.players = true,
                    "--land" => a.land = true,
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
    // Fingers are read as such (see Ui): one sliding pans, two pinch.
    simulate_mouse_with_touch(false);
    gfx::init_font();
    let mut bloom = Bloom::new();
    let mut lib = Library::load();
    let mut ui = Ui::default();
    let mut settings = Settings::load();
    if let Some(n) = &args.name {
        settings.name = n.clone();
    }

    let mut app = if let Some(level) = args.offline {
        let ais = args.ais.unwrap_or(6);
        let seed = args.seed.unwrap_or_else(platform::seed);
        let mut g = Game::offline(&lib, settings.clone(), seed, ais, level);
        if args.land {
            g.auto_land();
        }
        g.fast_forward(args.ticks);
        if let Some(z) = args.zoom {
            g.set_zoom(z);
        }
        if args.editor {
            g.open_program_by_name(&lib, "miner");
        }
        if args.debug_dronoid {
            g.debug_first_dronoid();
        }
        g.set_overlays(args.help, args.build, args.map, args.players);
        App::Game(Box::new(g))
    } else {
        let mut m = Menu::new(settings.clone());
        if let Some(url) = &args.join {
            m.connect(Some(url));
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
                MenuOut::Offline { ais, level, settings } => {
                    next = Some(App::Game(Box::new(Game::offline(&lib, settings, platform::seed(), ais, level))));
                }
                MenuOut::Online { conn, me, world, name } => {
                    let mut g = Game::online(&lib, conn, me, *world, name);
                    if let Some(z) = args.zoom {
                        g.set_zoom(z);
                    }
                    g.set_overlays(false, false, args.map, args.players);
                    next = Some(App::Game(Box::new(g)));
                }
            },
            App::Game(g) => {
                if let Outcome::ToMenu = g.frame(&mut ui, &mut lib, &mut bloom) {
                    next = Some(App::Menu(Box::new(Menu::new(Settings::load()))));
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
