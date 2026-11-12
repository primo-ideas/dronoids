//! Dronoids — program dronoid colonies in a tiny assembly language, in
//! a shared, persistent sector (or offline, for practice).

mod blocks;
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
    // `--size W,H`: another window size (a phone's, to check that layout).
    let args: Vec<String> = std::env::args().collect();
    let size = args.iter().position(|a| a == "--size").and_then(|i| args.get(i + 1)).and_then(|s| {
        let (w, h) = s.split_once(',')?;
        Some((w.parse().ok()?, h.parse().ok()?))
    });
    let (window_width, window_height) = size.unwrap_or((1600, 900));
    Conf {
        window_title: "Dronoids".to_owned(),
        window_width,
        window_height,
        window_resizable: true,
        high_dpi: false,
        sample_count: 4,
        ..Default::default()
    }
}

/// Command-line switches, mostly for quick testing:
/// `--offline [easy|normal|hard]`, `--ais N`, `--seed N`, `--ticks N`
/// (fast-forward), `--land` (land the base), `--zoom Z`,
/// `--editor`, `--debug-dronoid`, `--help-screen`, `--map`,
/// `--players`, `--join URL`, `--name NICKNAME`, `--perks` (the base gets
/// some), `--relic` (the same, looking at the nearest relic),
/// `--screenshot out.png [--after SECS | --frames N]`,
/// `--size W,H` (the window's size), `--click X,Y@FRAME` (a left click at that pixel on that frame; may be
/// given several times).
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
    map: bool,
    players: bool,
    land: bool,
    perks: bool,
    relic: bool,
    ticks: u64,
    zoom: Option<f32>,
    join: Option<String>,
    name: Option<String>,
    /// Scripted clicks: frame, then where.
    clicks: Vec<(u32, f32, f32)>,
}

/// `X,Y@FRAME`.
fn parse_click(s: &str) -> Option<(u32, f32, f32)> {
    let (at, frame) = s.split_once('@')?;
    let (x, y) = at.split_once(',')?;
    Some((frame.parse().ok()?, x.parse().ok()?, y.parse().ok()?))
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
            "--click" => a.clicks.extend(parse_click(&next)),
            // Read by `window_conf`.
            "--size" => {}
            flag => {
                take = false;
                match flag {
                    "--editor" => a.editor = true,
                    "--debug-dronoid" => a.debug_dronoid = true,
                    "--help-screen" => a.help = true,
                    "--map" => a.map = true,
                    "--players" => a.players = true,
                    "--land" => a.land = true,
                    "--perks" => a.perks = true,
                    "--relic" => a.relic = true,
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
        if args.perks || args.relic {
            g.demo_relics(args.relic);
        }
        if args.editor {
            g.open_program_by_name(&lib, "miner");
        }
        if args.debug_dronoid {
            g.debug_first_dronoid();
        }
        g.set_overlays(args.help, args.map, args.players);
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
        if let Some(&(_, x, y)) = args.clicks.iter().find(|c| c.0 == frame) {
            ui.inject_click(vec2(x, y));
        }
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
                    g.set_overlays(false, args.map, args.players);
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
