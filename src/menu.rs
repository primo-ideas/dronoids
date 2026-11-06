//! Title screen (with colonies living in the background) and connection.

use std::f32::consts::PI;

use macroquad::prelude::*;

use dronoids::ai::{Bot, Level};
use dronoids::net::{ClientMsg, Conn, Connecting, NetEvent, ServerMsg, PROTOCOL_VERSION};
use dronoids::rules::TICKS_PER_SEC;
use dronoids::sim::{Command, World};

use crate::game::bot_name;
use crate::gfx::{self, mix, pal, player_color, poly_lines, regular, with_alpha, Bloom};
use crate::platform::{self, Settings};
use crate::ui::{TextField, Ui};
use crate::view::{fp, Cam, Highlight, Sight, WorldView};

pub enum MenuOut {
    Stay,
    Offline { ais: usize, level: Level, settings: Settings },
    Online { conn: Conn<ServerMsg>, me: u8, world: Box<World>, name: String, settings: Settings },
    Quit,
}

enum Screen {
    Main,
    Connecting {
        connecting: Option<Connecting<ServerMsg>>,
        conn: Option<Conn<ServerMsg>>,
        status: String,
        error: bool,
    },
}

#[derive(PartialEq)]
enum Fold {
    None,
    Offline,
    Server,
}

pub struct Menu {
    demo: World,
    bots: Vec<Bot>,
    view: WorldView,
    acc: f32,
    pub settings: Settings,
    name: TextField,
    server: TextField,
    fold: Fold,
    level: Level,
    ais: usize,
    screen: Screen,
}

fn demo_world() -> (World, Vec<Bot>) {
    let mut w = World::new(platform::seed());
    let mut bots: Vec<Bot> = (0..10).filter_map(|i| w.join(&bot_name(i), true)).map(|id| Bot::new(id, Level::Hard)).collect();
    // Let the colonies settle before the title shows them.
    for _ in 0..500 {
        let mut cmds = vec![];
        for b in &mut bots {
            let p = b.player;
            cmds.extend(b.think(&w).into_iter().map(|c| (p, c)));
        }
        w.step(&cmds);
    }
    (w, bots)
}

impl Menu {
    pub fn new(settings: Settings) -> Menu {
        let (demo, bots) = demo_world();
        let name = TextField::new(&settings.name, 20, |c| !c.is_control());
        let server = TextField::new(&settings.server, 80, |c| !c.is_control() && c != ' ');
        Menu {
            demo,
            bots,
            view: WorldView::default(),
            acc: 0.0,
            settings,
            name,
            server,
            fold: Fold::None,
            level: Level::Normal,
            ais: 6,
            screen: Screen::Main,
        }
    }

    fn remember(&mut self) {
        let n = self.name.text.trim();
        if !n.is_empty() {
            self.settings.name = n.chars().take(20).collect();
        }
        self.settings.server = self.server.text.trim().to_string();
        self.settings.save();
    }

    /// Connect to the shared world (also used by `--join`).
    pub fn connect(&mut self, url: Option<&str>) {
        self.remember();
        let url = url.map(str::to_string).unwrap_or_else(|| self.settings.server_url());
        self.screen = Screen::Connecting {
            connecting: Some(Connecting::start(&url)),
            conn: None,
            status: "Connexion au secteur…".into(),
            error: false,
        };
    }

    fn step_demo(&mut self, dt: f32) {
        self.acc += dt * TICKS_PER_SEC * 3.0;
        let mut n = 0;
        while self.acc >= 1.0 && n < 6 {
            let mut cmds: Vec<(u8, Command)> = vec![];
            for b in &mut self.bots {
                let p = b.player;
                cmds.extend(b.think(&self.demo).into_iter().map(|c| (p, c)));
            }
            self.demo.step(&cmds);
            self.view.on_events(&self.demo, 3.0, &Sight::all());
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
        // A slow drift over one of the colonies.
        let t = self.view.time * 0.015;
        let size = fp(self.demo.size);
        let focus = self.demo.factories.first().map(|f| vec2(fp(f.x), fp(f.y))).unwrap_or(Vec2::splat(size / 2.0));
        let center = focus + vec2(40.0 * t.sin(), 25.0 * (t * 0.7).cos());
        let cam = Cam { center, zoom: 11.0, shake: Vec2::ZERO };
        let hl = Highlight { factory: None, drone: None, ghost: None, build_range_for: None, target: None, landing: None, me: 0 };
        self.view.draw(&self.demo, &cam, self.acc.clamp(0.0, 1.0), &hl, &Sight::all());
        draw_rectangle(0.0, 0.0, sw, sh, Color::new(0.01, 0.012, 0.03, 0.62));
        let ty = sh * 0.16;
        for i in 0..3 {
            let c = vec2(sw / 2.0 + (i as f32 - 1.0) * 40.0, ty - 30.0);
            let col = if i == 1 { pal::ORE } else { player_color(if i == 0 { 0 } else { 1 }) };
            poly_lines(&regular(c, 11.0, 6, PI / 6.0 + self.view.time * 0.4 * (i as f32 - 1.0)), 2.0, with_alpha(col, 0.85));
        }
        gfx::text_centered("DRONOIDS", sw / 2.0, ty, 54.0, mix(pal::ACCENT, WHITE, 0.2));
        gfx::text_centered("un secteur partagé · des drones que vous programmez", sw / 2.0, ty + 66.0, 15.0, with_alpha(pal::PINK, 0.85));
        bloom.finish();

        let out = match self.screen {
            Screen::Main => self.main_screen(ui),
            Screen::Connecting { .. } => self.connecting_screen(ui),
        };
        ui.end_frame();
        out
    }

    fn main_screen(&mut self, ui: &mut Ui) -> MenuOut {
        let (sw, sh) = (screen_width(), screen_height());
        let w = 340.0;
        let x = sw / 2.0 - w / 2.0;
        let mut y = (sh * 0.40).max(250.0);
        draw_rectangle(x - 24.0, y - 34.0, w + 48.0, 172.0, with_alpha(pal::PANEL, 0.7));

        gfx::text("pseudo", x, y - 18.0, 12.0, pal::DIM);
        let enter = self.name.ui(ui, Rect::new(x, y, w, 36.0), "votre nom");
        y += 48.0;
        if ui.button_ex(Rect::new(x, y, w, 42.0), "Entrer dans le secteur", false, true, pal::ACCENT) || enter {
            self.connect(None);
            return MenuOut::Stay;
        }
        y += 52.0;
        gfx::text_centered("Vous arrivez sans rien, de quoi bâtir une usine.", sw / 2.0, y, 12.0, pal::DIM);
        gfx::text_centered("Votre colonie travaille même quand vous partez.", sw / 2.0, y + 17.0, 12.0, pal::DIM);
        y += 50.0;

        let mut labels: Vec<(&str, Fold)> = vec![("Entraînement hors ligne", Fold::Offline), ("Serveur", Fold::Server)];
        if !platform::is_web() {
            labels.push(("Quitter", Fold::None));
        }
        let widths: Vec<f32> = labels.iter().map(|(l, _)| gfx::text_width(l, 13.0) + 22.0).collect();
        let total: f32 = widths.iter().sum::<f32>() + 8.0 * (labels.len() as f32 - 1.0);
        let mut bx = sw / 2.0 - total / 2.0;
        for ((label, fold), bw) in labels.into_iter().zip(widths) {
            let active = fold != Fold::None && self.fold == fold;
            if ui.button_ex(Rect::new(bx, y, bw, 28.0), label, active, true, pal::ACCENT) {
                if label == "Quitter" {
                    self.remember();
                    return MenuOut::Quit;
                }
                self.fold = if self.fold == fold { Fold::None } else { fold };
            }
            bx += bw + 8.0;
        }
        y += 44.0;

        match self.fold {
            Fold::Offline => {
                let r = Rect::new(x - 10.0, y, w + 20.0, 120.0);
                ui.panel(r);
                gfx::text("colonies IA", x, y + 16.0, 13.0, pal::TEXT);
                let bx = x + w - 110.0;
                if ui.button(Rect::new(bx, y + 10.0, 28.0, 26.0), "−") && self.ais > 0 {
                    self.ais -= 1;
                }
                gfx::text_centered(&format!("{}", self.ais), bx + 55.0, y + 13.0, 16.0, pal::ACCENT);
                if ui.button(Rect::new(bx + 82.0, y + 10.0, 28.0, 26.0), "+") && self.ais < 10 {
                    self.ais += 1;
                }
                let lw = (w - 16.0) / 3.0;
                for (i, l) in [Level::Easy, Level::Normal, Level::Hard].into_iter().enumerate() {
                    if ui.button_ex(Rect::new(x + i as f32 * (lw + 8.0), y + 46.0, lw, 26.0), l.label(), self.level == l, true, pal::ACCENT) {
                        self.level = l;
                    }
                }
                if ui.button_ex(Rect::new(x, y + 82.0, w, 28.0), "Lancer l'entraînement", false, true, pal::GOOD) {
                    self.remember();
                    return MenuOut::Offline { ais: self.ais, level: self.level, settings: self.settings.clone() };
                }
            }
            Fold::Server => {
                let r = Rect::new(x - 10.0, y, w + 20.0, 74.0);
                ui.panel(r);
                gfx::text("adresse du secteur (vide = par défaut)", x, y + 10.0, 12.0, pal::DIM);
                let default = platform::default_server();
                self.server.ui(ui, Rect::new(x, y + 30.0, w, 32.0), &default);
            }
            Fold::None => {}
        }
        MenuOut::Stay
    }

    fn connecting_screen(&mut self, ui: &mut Ui) -> MenuOut {
        let (sw, sh) = (screen_width(), screen_height());
        let r = Rect::new(sw / 2.0 - 230.0, (sh * 0.42).max(240.0), 460.0, 150.0);
        ui.panel(r);
        let name = self.settings.name.clone();
        let token = self.settings.token.clone();
        let settings = self.settings.clone();
        let Screen::Connecting { connecting, conn, status, error } = &mut self.screen else { return MenuOut::Stay };
        if let Some(c) = connecting
            && let Some(res) = c.poll()
        {
            *connecting = None;
            match res {
                Ok(mut cn) => {
                    cn.send(&ClientMsg::Hello { version: PROTOCOL_VERSION, name, token });
                    *conn = Some(cn);
                    *status = "Réception du secteur…".into();
                }
                Err(e) => {
                    *status = format!("Secteur injoignable : {e}");
                    *error = true;
                }
            }
        }
        if let Some(cn) = conn {
            // One message at a time: frames after the welcome stay queued for the game.
            while let Some(ev) = cn.poll_one() {
                match ev {
                    NetEvent::Msg(ServerMsg::Welcome { player, world, snapshot }) => match World::from_bytes(&snapshot) {
                        Ok(w) => {
                            let conn = conn.take().unwrap();
                            return MenuOut::Online { conn, me: player, world: Box::new(w), name: world, settings };
                        }
                        Err(e) => {
                            *status = format!("Secteur illisible : {e}");
                            *error = true;
                        }
                    },
                    NetEvent::Msg(ServerMsg::Refused(why)) => {
                        *status = why;
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
        gfx::text_centered(status, sw / 2.0, r.y + 36.0, 14.0, if *error { pal::WARN } else { pal::TEXT });
        if !*error {
            let dots = ".".repeat((get_time() * 2.0) as usize % 4);
            gfx::text_centered(&dots, sw / 2.0, r.y + 60.0, 18.0, pal::ACCENT);
        }
        if ui.button(Rect::new(sw / 2.0 - 70.0, r.y + 98.0, 140.0, 30.0), if *error { "Retour" } else { "Annuler" }) {
            self.screen = Screen::Main;
        }
        MenuOut::Stay
    }
}
