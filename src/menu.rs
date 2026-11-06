//! Main menu (with a live AI battle in the background), the world browser
//! and the connection screen.

use std::f32::consts::PI;
use std::path::PathBuf;

use macroquad::prelude::*;

use game_of_drones::ai::{Bot, Level};
use game_of_drones::net::{self, Browser, ClientMsg, Conn, Connecting, NetEvent, ServerMsg, DEFAULT_PORT, PROTOCOL_VERSION};
use game_of_drones::rules::TICKS_PER_SEC;
use game_of_drones::server::{bot_name, ServerConfig, ServerHandle};
use game_of_drones::sim::{Command, World};

use crate::game::{new_seed, player_name};
use crate::gfx::{self, mix, pal, player_color, poly_lines, regular, with_alpha, Bloom};
use crate::library;
use crate::ui::{TextField, Ui};
use crate::view::{fp, Cam, Highlight, WorldView};

pub enum MenuOut {
    Stay,
    Solo { ais: usize, level: Level },
    Online { conn: Conn<ServerMsg>, me: u8, world: Box<World>, name: String, server: Option<ServerHandle> },
    Quit,
}

enum Screen {
    Main,
    Worlds,
    Connecting {
        connecting: Option<Connecting>,
        conn: Option<Conn<ServerMsg>>,
        server: Option<ServerHandle>,
        status: String,
        error: bool,
    },
}

pub struct Menu {
    demo: World,
    bots: Vec<Bot>,
    view: WorldView,
    acc: f32,
    level: Level,
    ais: usize,
    host_ais: usize,
    screen: Screen,
    addr: TextField,
    name: TextField,
    world_name: TextField,
    browser: Option<Browser>,
    last_ping: f64,
}

fn demo_world() -> (World, Vec<Bot>) {
    let mut w = World::new(new_seed());
    let bots = (0..8).filter_map(|i| w.join(&bot_name(i), true)).map(|id| Bot::new(id, Level::Hard)).collect();
    (w, bots)
}

/// "label  [-] v [+]" on one line of width `r.w`.
fn stepper(v: usize, label: &str, ui: &mut Ui, r: Rect, range: std::ops::RangeInclusive<usize>) -> usize {
    let mut v = v;
    gfx::text(label, r.x, r.y + 8.0, 15.0, pal::TEXT);
    let bx = r.x + r.w - 130.0;
    if ui.button(Rect::new(bx, r.y, 36.0, 32.0), "-") && v > *range.start() {
        v -= 1;
    }
    gfx::text_centered(&format!("{v}"), bx + 65.0, r.y + 6.0, 20.0, pal::ACCENT);
    if ui.button(Rect::new(bx + 94.0, r.y, 36.0, 32.0), "+") && v < *range.end() {
        v += 1;
    }
    v
}

fn level_picker(level: Level, ui: &mut Ui, x: f32, y: f32, w: f32) -> Level {
    let mut out = level;
    let lw = (w - 16.0) / 3.0;
    for (i, l) in [Level::Easy, Level::Normal, Level::Hard].into_iter().enumerate() {
        let col = [pal::GOOD, pal::WARN, pal::BAD][i];
        if ui.button_ex(Rect::new(x + i as f32 * (lw + 8.0), y, lw, 30.0), l.label(), level == l, true, col) {
            out = l;
        }
    }
    out
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
            ais: 5,
            host_ais: 4,
            screen: Screen::Main,
            addr: TextField::new("", 60, |c| c.is_ascii_alphanumeric() || ".:-_[]".contains(c)),
            name: TextField::new(&player_name(), 20, |c| !c.is_control()),
            world_name: TextField::new("nebuleuse", 24, library::name_char_ok),
            browser: None,
            last_ping: -10.0,
        }
    }

    pub fn show_worlds(&mut self) {
        self.screen = Screen::Worlds;
    }

    pub fn set_player_name(&mut self, name: &str) {
        self.name.text = name.to_string();
    }

    /// Host a world and join it (also used by `--host`).
    pub fn host(&mut self, world: &str) {
        let name = library::sanitize(world);
        let cfg = ServerConfig {
            name: name.clone(),
            port: DEFAULT_PORT,
            seed: new_seed(),
            bots: self.host_ais,
            level: self.level,
            save: Some(PathBuf::from("worlds").join(format!("{name}.world"))),
        };
        match ServerHandle::spawn(cfg) {
            Ok(server) => {
                let addr = format!("127.0.0.1:{}", server.port);
                self.connect(&addr, Some(server));
            }
            Err(e) => {
                self.screen = Screen::Connecting {
                    connecting: None,
                    conn: None,
                    server: None,
                    status: format!("Impossible d'héberger sur le port {DEFAULT_PORT} : {e}"),
                    error: true,
                };
            }
        }
    }

    /// Connect to a world server (also used by `--join`).
    pub fn connect(&mut self, addr: &str, server: Option<ServerHandle>) {
        self.screen = Screen::Connecting {
            connecting: Some(Connecting::start(addr)),
            conn: None,
            server,
            status: format!("Connexion à {}…", net::with_default_port(addr)),
            error: false,
        };
    }

    fn step_demo(&mut self, dt: f32) {
        self.acc += dt * TICKS_PER_SEC * 4.0;
        let mut n = 0;
        while self.acc >= 1.0 && n < 10 {
            let mut cmds: Vec<(u8, Command)> = vec![];
            for b in &mut self.bots {
                let p = b.player;
                cmds.extend(b.think(&self.demo).into_iter().map(|c| (p, c)));
            }
            self.demo.step(&cmds);
            self.view.on_events(&self.demo, 4.0);
            self.acc -= 1.0;
            n += 1;
        }
        self.view.update(dt, &self.demo);
    }

    pub fn frame(&mut self, ui: &mut Ui, bloom: &mut Bloom) -> MenuOut {
        let dt = get_frame_time().min(0.1);
        self.step_demo(dt);
        let (sw, sh) = (screen_width(), screen_height());

        bloom.begin();
        // Slow fly-over of the demo world.
        let t = self.view.time * 0.02;
        let size = fp(self.demo.size);
        let center = vec2(size * (0.5 + 0.3 * t.sin()), size * (0.5 + 0.3 * (t * 0.7).cos()));
        let cam = Cam { center, zoom: 9.0, shake: Vec2::ZERO };
        let hl = Highlight { factory: None, drone: None, ghost: None, build_range_for: None, target: None, me: 0 };
        self.view.draw(&self.demo, &cam, self.acc.clamp(0.0, 1.0), &hl);
        draw_rectangle(0.0, 0.0, sw, sh, Color::new(0.01, 0.012, 0.03, 0.66));
        let ty = sh * 0.11;
        for i in 0..3 {
            let c = vec2(sw / 2.0 + (i as f32 - 1.0) * 46.0, ty - 34.0);
            let col = if i == 1 { pal::ORE } else { player_color(if i == 0 { 0 } else { 1 }) };
            poly_lines(&regular(c, 14.0, 6, PI / 6.0 + self.view.time * 0.5 * (i as f32 - 1.0)), 2.5, col);
        }
        gfx::text_centered("GAME OF DRONES", sw / 2.0, ty, 64.0, mix(pal::ACCENT, WHITE, 0.25));
        gfx::text_centered("programmez vos drones, minez, conquérez", sw / 2.0, ty + 76.0, 18.0, pal::PINK);
        bloom.finish();

        let out = match self.screen {
            Screen::Main => self.main_screen(ui),
            Screen::Worlds => self.worlds_screen(ui),
            Screen::Connecting { .. } => self.connecting_screen(ui),
        };
        gfx::text("programmes : ./programs   ·   mondes hébergés : ./worlds", 12.0, sh - 22.0, 12.0, pal::FAINT);
        ui.end_frame();
        out
    }

    fn main_screen(&mut self, ui: &mut Ui) -> MenuOut {
        let (sw, sh) = (screen_width(), screen_height());
        let w = 400.0;
        let x = sw / 2.0 - w / 2.0;
        let mut y = (sh * 0.30).max(200.0);
        ui.panel(Rect::new(x - 20.0, y - 20.0, w + 40.0, 384.0));

        if ui.button_ex(Rect::new(x, y, w, 48.0), "Solo : explorer un grand monde", false, true, pal::ACCENT) {
            return MenuOut::Solo { ais: self.ais, level: self.level };
        }
        y += 58.0;
        self.ais = stepper(self.ais, "IA adverses (0 = bac à sable)", ui, Rect::new(x, y, w, 32.0), 0..=8);
        y += 42.0;
        self.level = level_picker(self.level, ui, x, y, w);
        y += 54.0;
        if ui.button_ex(Rect::new(x, y, w, 48.0), "Multijoueur : mondes en réseau", false, true, pal::VIOLET) {
            self.screen = Screen::Worlds;
        }
        y += 58.0;
        gfx::text_centered("Les mondes hébergés persistent : vos drones y travaillent", sw / 2.0, y, 13.0, pal::DIM);
        gfx::text_centered("même quand vous n'êtes pas connecté.", sw / 2.0, y + 18.0, 13.0, pal::DIM);
        y += 54.0;
        if ui.button_ex(Rect::new(x, y, w, 40.0), "Quitter", false, true, pal::BAD) {
            return MenuOut::Quit;
        }
        MenuOut::Stay
    }

    fn worlds_screen(&mut self, ui: &mut Ui) -> MenuOut {
        let (sw, sh) = (screen_width(), screen_height());
        if self.browser.is_none() {
            self.browser = Browser::new().ok();
        }
        let now = get_time();
        if let Some(b) = &mut self.browser {
            if now - self.last_ping > 2.0 {
                b.ping();
                self.last_ping = now;
            }
            b.poll(now);
        }

        let total_w = (sw - 80.0).min(1000.0);
        let x0 = (sw - total_w) / 2.0;
        let top = (sh * 0.27).max(190.0);
        let h = (sh - top - 50.0).min(470.0);
        let list_w = total_w * 0.58;
        let lr = Rect::new(x0, top, list_w, h);
        let cr = Rect::new(x0 + list_w + 20.0, top, total_w - list_w - 20.0, h);
        ui.panel(lr);
        ui.panel(cr);

        // Player name.
        gfx::text("Votre nom", x0, top - 40.0, 14.0, pal::DIM);
        self.name.ui(ui, Rect::new(x0 + 90.0, top - 48.0, 220.0, 32.0), "pseudo");
        if ui.button(Rect::new(x0 + total_w - 120.0, top - 48.0, 120.0, 32.0), "Retour") {
            self.screen = Screen::Main;
            return MenuOut::Stay;
        }

        // Discovered worlds.
        gfx::text("MONDES SUR LE RÉSEAU LOCAL", lr.x + 16.0, lr.y + 14.0, 13.0, pal::VIOLET);
        let mut y = lr.y + 40.0;
        let mut join: Option<String> = None;
        let found = self.browser.as_ref().map(|b| b.found.clone()).unwrap_or_default();
        if found.is_empty() {
            let dots = ".".repeat((now * 2.0) as usize % 4);
            gfx::text(&format!("Recherche{dots}"), lr.x + 16.0, y + 6.0, 15.0, pal::DIM);
            gfx::text("Aucun monde trouvé pour l'instant : hébergez-en un, ou entrez", lr.x + 16.0, y + 34.0, 13.0, pal::FAINT);
            gfx::text("une adresse ci-dessous pour un serveur hors du réseau local.", lr.x + 16.0, y + 52.0, 13.0, pal::FAINT);
        }
        for f in found.iter().take(((h - 140.0) / 58.0) as usize) {
            let row = Rect::new(lr.x + 10.0, y, lr.w - 20.0, 52.0);
            draw_rectangle(row.x, row.y, row.w, row.h, with_alpha(pal::VIOLET, if ui.hover(row) { 0.12 } else { 0.05 }));
            gfx::text(&f.info.name, row.x + 12.0, row.y + 6.0, 18.0, pal::TEXT);
            let mins = f.info.tick as f32 / TICKS_PER_SEC / 60.0;
            let detail = format!(
                "{} joueur(s), {} en ligne, {} IA · {:.0} min · {}",
                f.info.players, f.info.online, f.info.bots, mins, f.addr
            );
            gfx::text(&detail, row.x + 12.0, row.y + 30.0, 12.0, pal::DIM);
            let ok = f.info.version == PROTOCOL_VERSION;
            if ui.button_ex(Rect::new(row.x + row.w - 112.0, row.y + 10.0, 100.0, 32.0), if ok { "Rejoindre" } else { "Version ≠" }, false, ok, pal::GOOD) {
                join = Some(f.addr.clone());
            }
            y += 58.0;
        }
        let ay = lr.y + lr.h - 52.0;
        gfx::text("Adresse", lr.x + 16.0, ay + 9.0, 14.0, pal::DIM);
        let enter = self.addr.ui(ui, Rect::new(lr.x + 90.0, ay, lr.w - 220.0, 34.0), "ex. 192.168.1.20");
        let can = !self.addr.text.trim().is_empty();
        if (ui.button_ex(Rect::new(lr.x + lr.w - 120.0, ay, 104.0, 34.0), "Rejoindre", false, can, pal::GOOD) || enter) && can {
            join = Some(self.addr.text.trim().to_string());
        }

        // Create a world.
        gfx::text("CRÉER UN MONDE", cr.x + 16.0, cr.y + 14.0, 13.0, pal::VIOLET);
        let cx = cr.x + 16.0;
        let cw = cr.w - 32.0;
        gfx::text("Nom", cx, cr.y + 50.0, 14.0, pal::DIM);
        self.world_name.ui(ui, Rect::new(cx + 50.0, cr.y + 42.0, cw - 50.0, 32.0), "nom du monde");
        self.host_ais = stepper(self.host_ais, "IA", ui, Rect::new(cx, cr.y + 90.0, cw, 32.0), 0..=10);
        self.level = level_picker(self.level, ui, cx, cr.y + 134.0, cw);
        let lines = [
            "Le monde tourne tant que le jeu reste ouvert,",
            "et il est sauvegardé : héberger à nouveau le",
            "même nom reprend là où il en était.",
            &format!("Port TCP {DEFAULT_PORT}. Serveur dédié : god-server."),
        ];
        for (i, l) in lines.iter().enumerate() {
            gfx::text(l, cx, cr.y + 186.0 + i as f32 * 18.0, 12.0, pal::DIM);
        }
        if ui.button_ex(Rect::new(cx, cr.y + cr.h - 56.0, cw, 40.0), "Héberger et jouer", false, true, pal::ACCENT) {
            let name = self.world_name.text.clone();
            self.host(&name);
            return MenuOut::Stay;
        }

        if let Some(addr) = join {
            self.connect(&addr, None);
        }
        MenuOut::Stay
    }

    fn connecting_screen(&mut self, ui: &mut Ui) -> MenuOut {
        let (sw, sh) = (screen_width(), screen_height());
        let r = Rect::new(sw / 2.0 - 280.0, (sh * 0.36).max(220.0), 560.0, 200.0);
        ui.panel(r);
        gfx::text_centered("CONNEXION", sw / 2.0, r.y + 18.0, 24.0, pal::VIOLET);
        let player = self.name.text.trim().to_string();
        let Screen::Connecting { connecting, conn, server, status, error } = &mut self.screen else { return MenuOut::Stay };
        if let Some(c) = connecting
            && let Some(res) = c.poll()
        {
            *connecting = None;
            match res.and_then(Conn::<ServerMsg>::new) {
                Ok(mut cn) => {
                    cn.send(&ClientMsg::Hello { version: PROTOCOL_VERSION, name: player.clone() });
                    *conn = Some(cn);
                    *status = "Connecté, réception du monde…".into();
                }
                Err(e) => {
                    *status = format!("Échec : {e}");
                    *error = true;
                }
            }
        }
        if let Some(cn) = conn {
            // One message at a time: frames following the welcome stay queued
            // for the game.
            while let Some(ev) = cn.poll_one() {
                match ev {
                    NetEvent::Msg(ServerMsg::Welcome { player, world, snapshot }) => match World::from_bytes(&snapshot) {
                        Ok(w) => {
                            let conn = conn.take().unwrap();
                            return MenuOut::Online { conn, me: player, world: Box::new(w), name: world, server: server.take() };
                        }
                        Err(e) => {
                            *status = format!("Monde illisible : {e}");
                            *error = true;
                        }
                    },
                    NetEvent::Msg(ServerMsg::Refused(why)) => {
                        *status = format!("Refusé : {why}");
                        *error = true;
                    }
                    NetEvent::Msg(_) => {}
                    NetEvent::Closed(e) => {
                        if !*error {
                            *status = format!("Déconnecté : {e}");
                            *error = true;
                        }
                        *conn = None;
                        break;
                    }
                }
            }
        }
        gfx::text_centered(status, sw / 2.0, r.y + 80.0, 15.0, if *error { pal::BAD } else { pal::TEXT });
        if !*error {
            let dots = ".".repeat((get_time() * 2.0) as usize % 4);
            gfx::text_centered(&dots, sw / 2.0, r.y + 104.0, 20.0, pal::ACCENT);
        }
        if ui.button(Rect::new(sw / 2.0 - 80.0, r.y + 140.0, 160.0, 40.0), if *error { "Retour" } else { "Annuler" }) {
            self.screen = Screen::Worlds;
        }
        MenuOut::Stay
    }
}
