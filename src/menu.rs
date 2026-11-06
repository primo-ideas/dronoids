//! Main menu with a live bot-vs-bot battle in the background, plus the LAN
//! lobby (host / join).

use std::f32::consts::PI;

use macroquad::prelude::*;

use game_of_drones::ai::{Bot, Level};
use game_of_drones::net::{self, Connecting, Listening, Msg, NetEvent, Peer, DEFAULT_PORT, PROTOCOL_VERSION};
use game_of_drones::programs;
use game_of_drones::rules::TICKS_PER_SEC;
use game_of_drones::sim::{Command, World};

use crate::game::new_seed;
use crate::gfx::{self, mix, pal, player_color, poly_lines, regular, Bloom};
use crate::ui::{TextField, Ui};
use crate::view::{Cam, Highlight, WorldView};

pub enum MenuOut {
    Stay,
    Solo(Option<Level>),
    Net { peer: Peer, seed: u64, names: Vec<String>, me: u8 },
    Quit,
}

enum Screen {
    Main,
    Host { listen: Option<Listening>, peer: Option<Peer>, seed: u64, error: Option<String> },
    Join { connecting: Option<Connecting>, peer: Option<Peer>, status: String, error: bool },
}

pub struct Menu {
    demo: World,
    bots: [Bot; 2],
    view: WorldView,
    acc: f32,
    level: Level,
    screen: Screen,
    addr: TextField,
    name: String,
}

fn demo_world() -> (World, [Bot; 2]) {
    let m = programs::builtin("eclaireur").unwrap();
    let w = World::new(new_seed(), &["A".into(), "B".into()], m.name, m.source);
    (w, [Bot::new(0, Level::Hard), Bot::new(1, Level::Hard)])
}

impl Menu {
    pub fn new() -> Menu {
        let (demo, bots) = demo_world();
        Menu {
            demo,
            bots,
            view: WorldView::default(),
            acc: 0.0,
            level: Level::Normal,
            screen: Screen::Main,
            addr: TextField::new("127.0.0.1", 40, |c| c.is_ascii_alphanumeric() || ".:-_[]".contains(c)),
            name: std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap_or_else(|_| "Joueur".into()),
        }
    }

    /// Start hosting right away (command line `--host`).
    pub fn host(&mut self) {
        let seed = new_seed();
        self.screen = match Listening::start(DEFAULT_PORT) {
            Ok(l) => Screen::Host { listen: Some(l), peer: None, seed, error: None },
            Err(e) => Screen::Host { listen: None, peer: None, seed, error: Some(format!("port {DEFAULT_PORT} : {e}")) },
        };
    }

    /// Connect right away (command line `--join ADDR`).
    pub fn join(&mut self, addr: &str) {
        self.addr.text = addr.to_string();
        self.screen = Screen::Join {
            connecting: Some(Connecting::start(addr)),
            peer: None,
            status: "Connexion…".into(),
            error: false,
        };
    }

    fn step_demo(&mut self, dt: f32) {
        self.acc += dt * TICKS_PER_SEC * 3.0;
        let mut n = 0;
        while self.acc >= 1.0 && n < 8 {
            let mut cmds: Vec<(u8, Command)> = vec![];
            for b in &mut self.bots {
                let p = b.player;
                cmds.extend(b.think(&self.demo).into_iter().map(|c| (p, c)));
            }
            self.demo.step(&cmds);
            self.view.on_events(&self.demo, 3.0);
            self.acc -= 1.0;
            n += 1;
        }
        if self.demo.winner().is_some() || self.demo.tick > 9000 {
            let (w, b) = demo_world();
            self.demo = w;
            self.bots = b;
            self.view = WorldView::default();
        }
        self.view.update(dt, &self.demo);
    }

    pub fn frame(&mut self, ui: &mut Ui, bloom: &mut Bloom) -> MenuOut {
        let dt = get_frame_time().min(0.1);
        self.step_demo(dt);
        let (sw, sh) = (screen_width(), screen_height());

        // Background battle + title, both glowing.
        bloom.begin();
        let zoom = (sw / self.demo.w as f32).max(sh / self.demo.h as f32) * 1.05;
        let cam = Cam { center: vec2(self.demo.w as f32, self.demo.h as f32) / 2.0, zoom, shake: Vec2::ZERO };
        let hl = Highlight { factory: None, drone: None, ghost: None, build_range_for: None };
        self.view.draw(&self.demo, &cam, self.acc.clamp(0.0, 1.0), &hl);
        draw_rectangle(0.0, 0.0, sw, sh, Color::new(0.01, 0.012, 0.03, 0.72));
        let t = self.view.time;
        let ty = sh * 0.14;
        for i in 0..3 {
            let c = vec2(sw / 2.0 + (i as f32 - 1.0) * 46.0, ty - 34.0);
            let col = if i == 1 { pal::ORE } else { player_color(if i == 0 { 0 } else { 1 }) };
            poly_lines(&regular(c, 14.0, 6, PI / 6.0 + t * 0.5 * (i as f32 - 1.0)), 2.5, col);
        }
        gfx::text_centered("GAME OF DRONES", sw / 2.0, ty, 64.0, mix(pal::ACCENT, WHITE, 0.25));
        gfx::text_centered("programmez vos drones, minez, conquérez", sw / 2.0, ty + 76.0, 18.0, pal::PINK);
        bloom.finish();

        let out = match self.screen {
            Screen::Main => self.main_screen(ui),
            Screen::Host { .. } => self.host_screen(ui),
            Screen::Join { .. } => self.join_screen(ui),
        };
        gfx::text(
            "Rust + macroquad  ·  programmes sauvegardés dans ./programs",
            12.0,
            sh - 22.0,
            12.0,
            pal::FAINT,
        );
        ui.end_frame();
        out
    }

    fn main_screen(&mut self, ui: &mut Ui) -> MenuOut {
        let (sw, sh) = (screen_width(), screen_height());
        let w = 380.0;
        let x = sw / 2.0 - w / 2.0;
        let mut y = sh * 0.36;
        let r = Rect::new(x - 20.0, y - 20.0, w + 40.0, 392.0);
        ui.panel(r);

        if ui.button_ex(Rect::new(x, y, w, 48.0), "Solo contre l'IA", false, true, pal::ACCENT) {
            return MenuOut::Solo(Some(self.level));
        }
        y += 56.0;
        let lw = (w - 16.0) / 3.0;
        for (i, l) in [Level::Easy, Level::Normal, Level::Hard].into_iter().enumerate() {
            let col = [pal::GOOD, pal::WARN, pal::BAD][i];
            if ui.button_ex(Rect::new(x + i as f32 * (lw + 8.0), y, lw, 30.0), l.label(), self.level == l, true, col) {
                self.level = l;
            }
        }
        y += 50.0;
        if ui.button_ex(Rect::new(x, y, w, 44.0), "Bac à sable (sans ennemi)", false, true, pal::GOOD) {
            return MenuOut::Solo(None);
        }
        y += 60.0;
        if ui.button_ex(Rect::new(x, y, w, 44.0), "Héberger une partie (LAN)", false, true, pal::VIOLET) {
            self.host();
        }
        y += 52.0;
        if ui.button_ex(Rect::new(x, y, w, 44.0), "Rejoindre une partie (LAN)", false, true, pal::VIOLET) {
            self.screen = Screen::Join { connecting: None, peer: None, status: String::new(), error: false };
            self.addr.focused = true;
        }
        y += 60.0;
        if ui.button_ex(Rect::new(x, y, w, 40.0), "Quitter", false, true, pal::BAD) {
            return MenuOut::Quit;
        }
        MenuOut::Stay
    }

    fn host_screen(&mut self, ui: &mut Ui) -> MenuOut {
        let (sw, sh) = (screen_width(), screen_height());
        let r = Rect::new(sw / 2.0 - 260.0, sh * 0.36 - 20.0, 520.0, 260.0);
        ui.panel(r);
        let Screen::Host { listen, peer, seed, error } = &mut self.screen else { return MenuOut::Stay };
        gfx::text_centered("HÉBERGER", sw / 2.0, r.y + 18.0, 24.0, pal::VIOLET);
        let mut status = String::new();
        if let Some(l) = listen
            && let Some(res) = l.poll() {
                match res.and_then(Peer::new) {
                    Ok(p) => *peer = Some(p),
                    Err(e) => *error = Some(e.to_string()),
                }
                *listen = None;
            }
        if let Some(p) = peer {
            status = "Joueur connecté, synchronisation…".into();
            while let Some(ev) = p.poll_one() {
                match ev {
                    NetEvent::Msg(Msg::Join { version, name }) => {
                        if version != PROTOCOL_VERSION {
                            *error = Some("version de jeu différente".into());
                        } else {
                            let names = vec![self.name.clone(), name];
                            p.send(&Msg::Start { seed: *seed, names: names.clone(), your_id: 1 });
                            let peer = peer.take().unwrap();
                            return MenuOut::Net { peer, seed: *seed, names, me: 0 };
                        }
                    }
                    NetEvent::Closed(e) => *error = Some(e),
                    _ => {}
                }
            }
        } else if listen.is_some() {
            let ip = net::local_ip().unwrap_or_else(|| "?".into());
            status = format!("En attente d'un joueur sur  {ip}:{DEFAULT_PORT}");
        }
        gfx::text_centered(&status, sw / 2.0, r.y + 80.0, 17.0, pal::TEXT);
        gfx::text_centered("L'autre joueur choisit « Rejoindre » et tape cette adresse.", sw / 2.0, r.y + 112.0, 14.0, pal::DIM);
        let dots = ".".repeat(((get_time() * 2.0) as usize) % 4);
        if error.is_none() {
            gfx::text_centered(&dots, sw / 2.0, r.y + 136.0, 20.0, pal::ACCENT);
        }
        if let Some(e) = error {
            gfx::text_centered(e, sw / 2.0, r.y + 150.0, 15.0, pal::BAD);
        }
        if ui.button(Rect::new(sw / 2.0 - 80.0, r.y + 196.0, 160.0, 40.0), "Retour") {
            self.screen = Screen::Main;
        }
        MenuOut::Stay
    }

    fn join_screen(&mut self, ui: &mut Ui) -> MenuOut {
        let (sw, sh) = (screen_width(), screen_height());
        let r = Rect::new(sw / 2.0 - 260.0, sh * 0.36 - 20.0, 520.0, 260.0);
        ui.panel(r);
        gfx::text_centered("REJOINDRE", sw / 2.0, r.y + 18.0, 24.0, pal::VIOLET);
        gfx::text("Adresse de l'hôte", r.x + 30.0, r.y + 64.0, 14.0, pal::DIM);
        let enter = self.addr.ui(ui, Rect::new(r.x + 30.0, r.y + 84.0, 330.0, 38.0), "ex. 192.168.1.20");
        let Screen::Join { connecting, peer, status, error } = &mut self.screen else { return MenuOut::Stay };
        let busy = connecting.is_some() || peer.is_some();
        if (ui.button_ex(Rect::new(r.x + 370.0, r.y + 84.0, 120.0, 38.0), "Connexion", false, !busy, pal::GOOD) || enter) && !busy {
            *connecting = Some(Connecting::start(self.addr.text.trim()));
            *status = "Connexion…".into();
            *error = false;
        }
        if let Some(c) = connecting
            && let Some(res) = c.poll() {
                *connecting = None;
                match res.and_then(Peer::new) {
                    Ok(mut p) => {
                        p.send(&Msg::Join { version: PROTOCOL_VERSION, name: self.name.clone() });
                        *peer = Some(p);
                        *status = "Connecté, en attente de l'hôte…".into();
                    }
                    Err(e) => {
                        *status = format!("Échec : {e}");
                        *error = true;
                    }
                }
            }
        if let Some(p) = peer {
            while let Some(ev) = p.poll_one() {
                match ev {
                    NetEvent::Msg(Msg::Start { seed, names, your_id }) => {
                        let peer = peer.take().unwrap();
                        return MenuOut::Net { peer, seed, names, me: your_id };
                    }
                    NetEvent::Closed(e) => {
                        *status = format!("Déconnecté : {e}");
                        *error = true;
                        *peer = None;
                        break;
                    }
                    _ => {}
                }
            }
        }
        let col = if *error { pal::BAD } else { pal::TEXT };
        gfx::text_centered(status, sw / 2.0, r.y + 146.0, 15.0, col);
        if ui.button(Rect::new(sw / 2.0 - 80.0, r.y + 196.0, 160.0, 40.0), "Retour") {
            self.screen = Screen::Main;
        }
        MenuOut::Stay
    }
}
