//! One running match: drives the simulation (solo or lockstep), handles
//! input and draws the HUD, panels, program editor and debugger.

use std::sync::Arc;

use macroquad::prelude::*;

use game_of_drones::ai::{Bot, Level};
use game_of_drones::lockstep::Lockstep;
use game_of_drones::net::{Msg, NetEvent, Peer};
use game_of_drones::programs;
use game_of_drones::rules::*;
use game_of_drones::sim::{Cell, Command, LastAction, World};
use game_of_drones::vm::{self, Cat, Reg};

use crate::editor::{token_color, Editor};
use crate::gfx::{self, mix, pal, player_color, with_alpha, Bloom};
use crate::library::{self, Library};
use crate::ui::{TextField, Ui};
use crate::view::{draw_minimap, tile_center, Cam, Highlight, WorldView};

pub enum Driver {
    Solo { bot: Option<Bot>, pending: Vec<Command> },
    Net { peer: Peer, lock: Lockstep, lost: Option<String> },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sel {
    None,
    Factory(u32),
    Drone(u32),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditTarget {
    Program,
    Debug(u32),
}

pub enum Outcome {
    Continue,
    ToMenu,
}

enum Act {
    Issue(Command),
    OpenProgram(String, Option<u32>),
    OpenDebug(u32),
    Select(Sel),
    Speed(f32),
    Step,
    ToggleBuild,
    ToggleHelp,
    ToggleProgramMenu(u32),
    CloseEditor,
    Apply,
    NewProgram,
    Duplicate,
    ResetBuiltin,
    LoadProgram(String),
    CenterOn(Vec2),
    Follow(bool),
    DismissOver,
    Menu,
}

struct Toast {
    text: String,
    color: Color,
    t: f32,
}

struct EditorLayout {
    win: Rect,
    header: Rect,
    list: Rect,
    code: Rect,
    side: Rect,
    footer: Rect,
}

pub struct Game {
    pub world: World,
    pub me: u8,
    driver: Driver,
    pub cam: Cam,
    speed: f32,
    acc: f32,
    view: WorldView,
    sel: Sel,
    building: bool,
    editor_open: bool,
    editor: Editor,
    edit_name: TextField,
    edit_target: EditTarget,
    edit_factory: Option<u32>,
    debug_prog: usize,
    help_open: bool,
    program_menu: Option<u32>,
    toasts: Vec<Toast>,
    follow: bool,
    over_dismissed: bool,
    step_once: bool,
    right_drag: Option<(Vec2, Vec2)>,
    title: String,
}

fn player_name() -> String {
    std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap_or_else(|_| "Joueur".into())
}

pub fn new_seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1234)
}

impl Game {
    fn new(world: World, me: u8, driver: Driver, lib: &Library, title: String) -> Game {
        let start = world.factories.iter().find(|f| f.owner == me).map(|f| (f.id, f.x, f.y));
        let center = start.map(|(_, x, y)| tile_center(x, y)).unwrap_or(vec2(world.w as f32 / 2.0, world.h as f32 / 2.0));
        let mut g = Game {
            world,
            me,
            driver,
            cam: Cam { center, zoom: 30.0, shake: Vec2::ZERO },
            speed: 1.0,
            acc: 0.0,
            view: WorldView::default(),
            sel: start.map(|(id, _, _)| Sel::Factory(id)).unwrap_or(Sel::None),
            building: false,
            editor_open: false,
            editor: Editor::new(),
            edit_name: TextField::new("", 24, library::name_char_ok),
            edit_target: EditTarget::Program,
            edit_factory: None,
            debug_prog: 0,
            help_open: false,
            program_menu: None,
            toasts: vec![],
            follow: false,
            over_dismissed: false,
            step_once: false,
            right_drag: None,
            title,
        };
        // The starting factory runs the player's own version of "mineur".
        if let (Some((fid, _, _)), Some(src)) = (start, lib.source_or_builtin("mineur"))
            && Some(src.as_str()) != programs::builtin("mineur").map(|b| b.source) {
                g.issue(Command::SetProgram { factory: fid, name: "mineur".into(), source: src });
            }
        g.toast("Cliquez sur votre usine, puis « Éditer » pour programmer vos drones.", pal::ACCENT);
        g
    }

    pub fn solo(lib: &Library, seed: u64, level: Option<Level>) -> Game {
        let m = programs::builtin("mineur").unwrap();
        let mut names = vec![player_name()];
        if let Some(l) = level {
            names.push(format!("IA {}", l.label().to_lowercase()));
        }
        let world = World::new(seed, &names, m.name, m.source);
        let bot = level.map(|l| Bot::new(1, l));
        let title = match level {
            Some(l) => format!("Solo vs IA ({})", l.label()),
            None => "Bac à sable".into(),
        };
        Game::new(world, 0, Driver::Solo { bot, pending: vec![] }, lib, title)
    }

    pub fn net(lib: &Library, seed: u64, names: Vec<String>, me: u8, peer: Peer) -> Game {
        let m = programs::builtin("mineur").unwrap();
        let world = World::new(seed, &names, m.name, m.source);
        let other = 1 - me;
        let title = format!("En ligne vs {}", names.get(other as usize).cloned().unwrap_or_default());
        Game::new(world, me, Driver::Net { peer, lock: Lockstep::new(me, other), lost: None }, lib, title)
    }

    fn is_net(&self) -> bool {
        matches!(self.driver, Driver::Net { .. })
    }

    fn issue(&mut self, cmd: Command) {
        match &mut self.driver {
            Driver::Solo { pending, .. } => pending.push(cmd),
            Driver::Net { lock, .. } => lock.queue(cmd),
        }
    }

    fn toast(&mut self, text: impl Into<String>, color: Color) {
        self.toasts.push(Toast { text: text.into(), color, t: 0.0 });
        if self.toasts.len() > 5 {
            self.toasts.remove(0);
        }
    }

    pub fn leave(&mut self) {
        if let Driver::Net { peer, .. } = &mut self.driver {
            peer.send(&Msg::Bye);
        }
    }

    // ----- simulation ------------------------------------------------------

    fn advance(&mut self, dt: f32) {
        match &mut self.driver {
            Driver::Solo { bot, pending } => {
                if self.speed <= 0.0 && !self.step_once {
                    return;
                }
                self.acc += dt * self.speed * TICKS_PER_SEC;
                if self.step_once {
                    self.acc = self.acc.max(1.0);
                    self.step_once = false;
                }
                let mut n = 0;
                while self.acc >= 1.0 {
                    let mut cmds: Vec<(u8, Command)> = pending.drain(..).map(|c| (self.me, c)).collect();
                    if let Some(b) = bot {
                        let p = b.player;
                        cmds.extend(b.think(&self.world).into_iter().map(|c| (p, c)));
                    }
                    self.world.step(&cmds);
                    self.view.on_events(&self.world, self.speed);
                    self.acc -= 1.0;
                    n += 1;
                    if n >= 48 {
                        self.acc = self.acc.min(0.99);
                        break;
                    }
                }
                if self.speed <= 0.0 {
                    self.acc = 0.999;
                }
            }
            Driver::Net { peer, lock, lost } => {
                for ev in peer.poll() {
                    match ev {
                        NetEvent::Msg(Msg::Turn(t)) => lock.receive(t),
                        NetEvent::Msg(Msg::Bye) => *lost = Some("L'adversaire a quitté la partie.".into()),
                        NetEvent::Msg(_) => {}
                        NetEvent::Closed(e) => {
                            if lost.is_none() {
                                *lost = Some(format!("Connexion perdue : {e}"));
                            }
                        }
                    }
                }
                if lost.is_some() {
                    return;
                }
                self.acc += dt * TICKS_PER_SEC;
                let mut n = 0;
                loop {
                    let world = &self.world;
                    if let Some(t) = lock.outgoing(world.tick, || world.state_hash()) {
                        peer.send(&Msg::Turn(t));
                    }
                    if self.acc < 1.0 || n >= 8 {
                        break;
                    }
                    match lock.ready(self.world.tick) {
                        Some(cmds) => {
                            self.world.step(&cmds);
                            self.view.on_events(&self.world, 1.0);
                            self.acc -= 1.0;
                            n += 1;
                        }
                        None => {
                            self.acc = self.acc.min(1.0);
                            break;
                        }
                    }
                }
            }
        }
    }

    fn alpha(&self) -> f32 {
        self.acc.clamp(0.0, 1.0)
    }

    // ----- frame ----------------------------------------------------------

    pub fn frame(&mut self, ui: &mut Ui, lib: &mut Library, bloom: &mut Bloom) -> Outcome {
        let dt = get_frame_time().min(0.1);
        self.advance(dt);
        self.view.update(dt, &self.world);
        for t in &mut self.toasts {
            t.t += dt;
        }
        self.toasts.retain(|t| t.t < 4.5);
        self.validate_selection();

        if self.follow
            && let Sel::Drone(id) = self.sel
                && let Some(p) = WorldView::drone_pos(&self.world, id, self.alpha()) {
                    self.cam.center = self.cam.center.lerp(p, (dt * 6.0).min(1.0));
                }
        self.cam.shake = self.view.shake_offset();

        // World (with bloom).
        bloom.begin();
        let hl = self.highlight(ui);
        self.view.draw(&self.world, &self.cam, self.alpha(), &hl);
        bloom.finish();

        // HUD.
        let mut acts = vec![];
        self.top_bar(ui, &mut acts);
        self.toolbar(ui, &mut acts);
        if self.editor_open {
            self.editor_window(ui, lib, &mut acts);
        } else {
            self.selection_panel(ui, lib, &mut acts);
            let mm = Rect::new(screen_width() - 230.0, screen_height() - 160.0, 220.0, 150.0);
            ui.block(mm);
            if let Some(p) = draw_minimap(&self.world, &self.cam, mm, ui.mouse, ui.pressed || (ui.down && ui.hover(mm))) {
                ui.pressed = false;
                acts.push(Act::CenterOn(p));
            }
        }
        self.draw_toasts();
        self.overlays(ui, &mut acts);
        if self.help_open {
            help_overlay(ui, &mut acts);
        }

        self.world_input(ui, lib, &mut acts);
        self.hotkeys(ui, &mut acts);
        ui.end_frame();

        let mut out = Outcome::Continue;
        for a in acts {
            if let Some(o) = self.act(a, lib) {
                out = o;
            }
        }
        out
    }

    fn validate_selection(&mut self) {
        match self.sel {
            Sel::Factory(id) if self.world.factory(id).is_none() => self.sel = Sel::None,
            Sel::Drone(id) if self.world.drone(id).is_none() => {
                self.sel = Sel::None;
                self.follow = false;
            }
            _ => {}
        }
        if let Some(fid) = self.program_menu
            && self.sel != Sel::Factory(fid) {
                self.program_menu = None;
            }
    }

    fn highlight(&self, ui: &Ui) -> Highlight {
        let ghost = if self.building && !ui.over_ui() {
            let (x, y) = self.cam.tile_at(ui.mouse);
            Some(((x, y), self.world.can_place_factory(self.me, x, y).is_ok()))
        } else {
            None
        };
        Highlight {
            factory: if let Sel::Factory(id) = self.sel { Some(id) } else { None },
            drone: if let Sel::Drone(id) = self.sel { Some(id) } else { None },
            ghost,
            build_range_for: self.building.then_some(self.me),
        }
    }

    fn act(&mut self, a: Act, lib: &mut Library) -> Option<Outcome> {
        match a {
            Act::Issue(c) => self.issue(c),
            Act::OpenProgram(name, factory) => self.open_program(lib, &name, factory),
            Act::OpenDebug(id) => self.open_debug(id),
            Act::Select(s) => {
                self.sel = s;
                self.program_menu = None;
                if !matches!(s, Sel::Drone(_)) {
                    self.follow = false;
                }
            }
            Act::Speed(s) => {
                if !self.is_net() {
                    self.speed = s;
                }
            }
            Act::Step => {
                if !self.is_net() {
                    self.speed = 0.0;
                    self.step_once = true;
                }
            }
            Act::ToggleBuild => {
                self.building = !self.building;
                if self.building {
                    self.toast("Construction : clic gauche pour poser (Maj = plusieurs), clic droit pour annuler.", pal::GOOD);
                }
            }
            Act::ToggleHelp => self.help_open = !self.help_open,
            Act::ToggleProgramMenu(fid) => {
                self.program_menu = if self.program_menu == Some(fid) { None } else { Some(fid) };
            }
            Act::CloseEditor => {
                self.editor_open = false;
                self.editor.focused = false;
                self.edit_name.focused = false;
            }
            Act::Apply => self.apply(lib),
            Act::NewProgram => match lib.create() {
                Ok(name) => self.open_program(lib, &name, None),
                Err(e) => self.toast(e, pal::BAD),
            },
            Act::Duplicate => {
                let name = lib.unique_name(&self.edit_name.text);
                let src = self.editor.text();
                match lib.save(&name, &src) {
                    Ok(n) => {
                        self.open_program(lib, &n, self.edit_factory);
                        self.toast(format!("Copie créée : « {n} »"), pal::GOOD);
                    }
                    Err(e) => self.toast(e, pal::BAD),
                }
            }
            Act::ResetBuiltin => {
                let name = self.edit_name.text.clone();
                if let Some(src) = Library::builtin_source(&name) {
                    self.editor.set_text(src);
                    self.editor.dirty = true;
                    self.toast("Version d'origine restaurée (pas encore appliquée).", pal::WARN);
                }
            }
            Act::LoadProgram(name) => {
                if self.editor.dirty {
                    self.toast("Modifications non appliquées abandonnées.", pal::WARN);
                }
                self.open_program(lib, &name, self.edit_factory);
            }
            Act::CenterOn(p) => {
                self.cam.center = p;
                self.follow = false;
            }
            Act::Follow(f) => self.follow = f,
            Act::DismissOver => self.over_dismissed = true,
            Act::Menu => {
                self.leave();
                return Some(Outcome::ToMenu);
            }
        }
        None
    }

    // ----- editor ---------------------------------------------------------

    fn open_program(&mut self, lib: &Library, name: &str, factory: Option<u32>) {
        let src = lib
            .source_or_builtin(name)
            .or_else(|| factory.and_then(|f| self.world.factory(f)).map(|f| f.program.source.clone()))
            .unwrap_or_else(|| programs::TEMPLATE.to_string());
        self.editor.set_text(&src);
        self.editor.read_only = false;
        self.editor.exec_line = None;
        self.edit_name.text = name.to_string();
        self.edit_target = EditTarget::Program;
        self.edit_factory = factory;
        self.editor_open = true;
        self.editor.focused = true;
        self.building = false;
    }

    fn open_debug(&mut self, id: u32) {
        let Some(d) = self.world.drone(id) else { return };
        self.editor.set_text(&d.program.source);
        self.editor.read_only = true;
        self.edit_name.text = d.program.name.clone();
        self.debug_prog = Arc::as_ptr(&d.program) as usize;
        self.edit_target = EditTarget::Debug(id);
        self.edit_factory = Some(d.factory);
        self.editor_open = true;
        self.editor.focused = false;
        self.sel = Sel::Drone(id);
        self.follow = true;
    }

    fn apply(&mut self, lib: &mut Library) {
        if self.edit_target != EditTarget::Program {
            return;
        }
        let src = self.editor.text();
        let name = match lib.save(&self.edit_name.text, &src) {
            Ok(n) => n,
            Err(e) => {
                self.toast(e, pal::BAD);
                library::sanitize(&self.edit_name.text)
            }
        };
        self.edit_name.text = name.clone();
        match vm::assemble(&name, &src) {
            Ok(_) => {
                self.issue(Command::UpdateProgram { name: name.clone(), source: src.clone() });
                let mut extra = String::new();
                if let Some(fid) = self.edit_factory
                    && let Some(f) = self.world.factory(fid)
                        && f.owner == self.me && f.program.name != name {
                            self.issue(Command::SetProgram { factory: fid, name: name.clone(), source: src });
                            extra = format!(" et assigné à l'usine #{fid}");
                        }
                self.editor.dirty = false;
                self.toast(format!("« {name} » sauvegardé, appliqué{extra}."), pal::GOOD);
            }
            Err(errs) => {
                self.toast(format!("Sauvegardé, mais {} erreur(s) : non appliqué.", errs.len()), pal::BAD);
            }
        }
    }

    fn editor_layout(&self) -> EditorLayout {
        let (sw, sh) = (screen_width(), screen_height());
        let x0 = (sw * 0.36).max(sw - 1040.0).min(sw - 560.0).max(10.0);
        let win = Rect::new(x0, 54.0, sw - x0 - 10.0, sh - 54.0 - 70.0);
        let header = Rect::new(win.x + 8.0, win.y + 8.0, win.w - 16.0, 34.0);
        let footer = Rect::new(win.x + 8.0, win.y + win.h - 78.0, win.w - 16.0, 70.0);
        let body_y = header.y + header.h + 8.0;
        let body_h = footer.y - 8.0 - body_y;
        let debug = matches!(self.edit_target, EditTarget::Debug(_));
        let list_w = if debug { 0.0 } else { 150.0 };
        let side_w = if debug { 200.0 } else { 0.0 };
        let list = Rect::new(win.x + 8.0, body_y, list_w, body_h);
        let code_x = list.x + list_w + if debug { 0.0 } else { 8.0 };
        let code = Rect::new(code_x, body_y, win.x + win.w - 8.0 - side_w - code_x - if debug { 8.0 } else { 0.0 }, body_h);
        let side = Rect::new(win.x + win.w - 8.0 - side_w, body_y, side_w, body_h);
        EditorLayout { win, header, list, code, side, footer }
    }

    fn editor_window(&mut self, ui: &mut Ui, lib: &Library, acts: &mut Vec<Act>) {
        let l = self.editor_layout();
        ui.panel(l.win);
        let debug_id = if let EditTarget::Debug(id) = self.edit_target { Some(id) } else { None };

        // Keep the debugger in sync with the drone.
        if let Some(id) = debug_id {
            match self.world.drone(id) {
                Some(d) => {
                    let ptr = Arc::as_ptr(&d.program) as usize;
                    if ptr != self.debug_prog {
                        self.debug_prog = ptr;
                        let scroll = self.editor.scroll;
                        self.editor.set_text(&d.program.source);
                        self.editor.scroll = scroll;
                        self.edit_name.text = d.program.name.clone();
                    }
                    self.editor.exec_line = d.program.line_of(d.vm.last_pc);
                }
                None => self.editor.exec_line = None,
            }
        }

        // Header.
        let h = l.header;
        let mut x = h.x;
        let bh = h.h;
        if let Some(id) = debug_id {
            let alive = self.world.drone(id).is_some();
            let title = if alive { format!("DÉBOGAGE  drone #{id}") } else { format!("DÉBOGAGE  drone #{id} (détruit)") };
            gfx::text(&title, x, h.y + 8.0, 18.0, if alive { pal::ACCENT } else { pal::BAD });
            x += gfx::text_width(&title, 18.0) + 16.0;
            gfx::text(&format!("« {} »", self.edit_name.text), x, h.y + 10.0, 15.0, pal::TEXT);
            let mut bx = h.x + h.w - 34.0;
            if ui.button(Rect::new(bx, h.y, 34.0, bh), "X") {
                acts.push(Act::CloseEditor);
            }
            bx -= 150.0;
            if ui.button_ex(Rect::new(bx, h.y, 144.0, bh), "Éditer le code", false, true, pal::GOOD) {
                acts.push(Act::OpenProgram(self.edit_name.text.clone(), self.edit_factory));
            }
            if !self.is_net() {
                bx -= 136.0;
                if ui.button_ex(Rect::new(bx, h.y, 130.0, bh), "Tick suivant [N]", false, true, pal::WARN) {
                    acts.push(Act::Step);
                }
            }
        } else {
            gfx::text("PROGRAMME", x, h.y + 9.0, 16.0, pal::DIM);
            x += 100.0;
            self.edit_name.ui(ui, Rect::new(x, h.y, 190.0, bh), "nom");
            if self.edit_name.focused {
                self.editor.focused = false;
            }
            x += 196.0;
            if self.editor.dirty {
                draw_circle(x + 6.0, h.y + bh / 2.0, 5.0, pal::WARN);
                if ui.hover(Rect::new(x, h.y, 14.0, bh)) {
                    ui.tooltip("Modifications non appliquées");
                }
            }
            x += 20.0;
            if ui.button_ex(Rect::new(x, h.y, 150.0, bh), "Appliquer  Ctrl+S", false, true, pal::GOOD) {
                acts.push(Act::Apply);
            }
            x += 156.0;
            let rest = [
                ("Nouveau", Act::NewProgram, true),
                ("Dupliquer", Act::Duplicate, true),
                ("Origine", Act::ResetBuiltin, Library::is_builtin(&self.edit_name.text)),
            ];
            for (label, act, enabled) in rest {
                let w = gfx::text_width(label, 14.0) + 22.0;
                if x + w > h.x + h.w - 80.0 {
                    break;
                }
                if ui.button_ex(Rect::new(x, h.y, w, bh), label, false, enabled, pal::ACCENT) {
                    acts.push(act);
                }
                x += w + 6.0;
            }
            if ui.button(Rect::new(h.x + h.w - 34.0, h.y, 34.0, bh), "X") {
                acts.push(Act::CloseEditor);
            }
            if ui.button(Rect::new(h.x + h.w - 74.0, h.y, 34.0, bh), "?") {
                acts.push(Act::ToggleHelp);
            }

            // Program list.
            let lr = l.list;
            draw_rectangle(lr.x, lr.y, lr.w, lr.h, with_alpha(BLACK, 0.35));
            gfx::text("BIBLIOTHÈQUE", lr.x + 8.0, lr.y + 6.0, 12.0, pal::DIM);
            let mut y = lr.y + 26.0;
            for e in &lib.entries {
                if y + 26.0 > lr.y + lr.h {
                    break;
                }
                let r = Rect::new(lr.x + 4.0, y, lr.w - 8.0, 24.0);
                let current = e.name == self.edit_name.text;
                let used = self.world.factories.iter().filter(|f| f.owner == self.me && f.program.name == e.name).count();
                if current {
                    draw_rectangle(r.x, r.y, r.w, r.h, with_alpha(pal::ACCENT, 0.18));
                } else if ui.hover(r) {
                    draw_rectangle(r.x, r.y, r.w, r.h, with_alpha(pal::ACCENT, 0.08));
                }
                let col = if current { pal::ACCENT } else { pal::TEXT };
                gfx::text(&e.name, r.x + 6.0, r.y + 5.0, 14.0, col);
                if used > 0 {
                    let s = format!("x{used}");
                    gfx::text(&s, r.x + r.w - gfx::text_width(&s, 12.0) - 6.0, r.y + 6.0, 12.0, pal::GOOD);
                }
                if !current && ui.click(r) {
                    acts.push(Act::LoadProgram(e.name.clone()));
                }
                y += 26.0;
            }
        }

        // Code.
        if self.editor.update(ui, l.code) {
            acts.push(Act::Apply);
        }
        if self.editor.focused {
            self.edit_name.focused = false;
        }
        if debug_id.is_some()
            && let Some(line) = self.editor.exec_line {
                self.editor.reveal(l.code, line);
            }
        if debug_id.is_none() {
            let src = self.editor.text();
            self.editor.errors = match vm::assemble(&self.edit_name.text, &src) {
                Ok(_) => vec![],
                Err(e) => e,
            };
        }
        self.editor.draw(ui, l.code);

        // Debug side column.
        if let Some(id) = debug_id {
            self.debug_side(ui, l.side, id, acts);
        }

        // Footer.
        let f = l.footer;
        draw_rectangle(f.x, f.y, f.w, f.h, with_alpha(BLACK, 0.35));
        if debug_id.is_some() {
            gfx::text(
                "La ligne en surbrillance est la dernière action exécutée par ce drone.",
                f.x + 10.0,
                f.y + 10.0,
                14.0,
                pal::DIM,
            );
            gfx::text(
                "Mettez en pause (Espace) et avancez tick par tick (N) pour suivre son raisonnement.",
                f.x + 10.0,
                f.y + 32.0,
                14.0,
                pal::DIM,
            );
        } else if self.editor.errors.is_empty() {
            let n = vm::assemble("x", &self.editor.text()).map(|p| p.code.len()).unwrap_or(0);
            gfx::text(&format!("OK  {n} instructions"), f.x + 10.0, f.y + 10.0, 15.0, pal::GOOD);
            let users = self.world.factories.iter().filter(|fa| fa.owner == self.me && fa.program.name == self.edit_name.text).count();
            let msg = if users > 0 {
                format!("Appliquer met à jour {users} usine(s) qui utilisent « {} » et tous leurs drones.", self.edit_name.text)
            } else if let Some(fid) = self.edit_factory {
                format!("Appliquer assigne ce programme à l'usine #{fid}.")
            } else {
                "Aucune usine n'utilise ce programme. Assignez-le depuis le panneau d'une usine.".into()
            };
            gfx::text(&msg, f.x + 10.0, f.y + 32.0, 13.0, pal::DIM);
        } else {
            let n = self.editor.errors.len();
            gfx::text(&format!("{n} erreur(s)"), f.x + 10.0, f.y + 8.0, 15.0, pal::BAD);
            for (i, e) in self.editor.errors.iter().take(2).enumerate() {
                let s = format!("ligne {} : {}", e.line + 1, e.msg);
                gfx::text(&s, f.x + 10.0, f.y + 28.0 + i as f32 * 18.0, 13.0, mix(pal::BAD, WHITE, 0.4));
            }
        }
    }

    fn debug_side(&self, ui: &mut Ui, r: Rect, id: u32, acts: &mut Vec<Act>) {
        draw_rectangle(r.x, r.y, r.w, r.h, with_alpha(BLACK, 0.35));
        let Some(d) = self.world.drone(id) else {
            gfx::text("Drone détruit.", r.x + 10.0, r.y + 10.0, 15.0, pal::BAD);
            return;
        };
        let mut y = r.y + 8.0;
        let x = r.x + 10.0;
        gfx::text("REGISTRES", x, y, 12.0, pal::DIM);
        y += 20.0;
        for reg in Reg::ALL.iter().chain([Reg::Ok].iter()) {
            let v = d.vm.reg(*reg);
            gfx::text(reg.name(), x, y, 18.0, token_color(vm::TokKind::Reg));
            let vs = describe_value(v);
            gfx::text(&vs, x + 40.0, y, 18.0, pal::TEXT);
            y += 24.0;
        }
        y += 8.0;
        gfx::text("ÉTAT", x, y, 12.0, pal::DIM);
        y += 20.0;
        let state = match d.last {
            LastAction::Idle => "vient de naître".to_string(),
            LastAction::Move => "avance".into(),
            LastAction::Mine(..) => "mine".into(),
            LastAction::Drop(..) => "décharge".into(),
            LastAction::Attack(..) => "attaque".into(),
            LastAction::Wait => "attend".into(),
            LastAction::Fail => "action échouée".into(),
            LastAction::Stall => format!("réfléchit (> {MAX_OPS_PER_TICK} instr.)"),
        };
        let col = match d.last {
            LastAction::Fail => pal::BAD,
            LastAction::Stall => pal::WARN,
            _ => pal::TEXT,
        };
        gfx::text(&state, x, y, 14.0, col);
        y += 22.0;
        let rows = [
            ("PV", format!("{}/{}", d.hp, DRONE_HP)),
            ("soute", format!("{}/{}", d.cargo, CARGO_CAP)),
            ("instr/tick", format!("{}", d.vm.last_ops)),
            ("position", format!("{},{}", d.x, d.y)),
        ];
        for (k, v) in rows {
            gfx::text(k, x, y, 13.0, pal::DIM);
            gfx::text(&v, x + 90.0, y, 13.0, pal::TEXT);
            y += 19.0;
        }
        y += 8.0;
        gfx::text("MÉMOIRE DE RUCHE", x, y, 12.0, pal::DIM);
        y += 18.0;
        if let Some(f) = self.world.factory(d.factory) {
            let cw = (r.w - 20.0) / 4.0;
            for i in 0..HIVE_MEM {
                let cx = x + (i % 4) as f32 * cw;
                let cy = y + (i / 4) as f32 * 26.0;
                let v = f.mem[i];
                let c = if v != 0 { pal::ACCENT } else { pal::FAINT };
                draw_rectangle_lines(cx, cy, cw - 4.0, 22.0, 1.0, with_alpha(c, 0.6));
                gfx::text(&format!("{v}"), cx + 4.0, cy + 4.0, 13.0, if v != 0 { pal::TEXT } else { pal::DIM });
            }
            y += 4.0 * 26.0 + 6.0;
        } else {
            gfx::text("(usine détruite)", x, y, 13.0, pal::DIM);
            y += 22.0;
        }
        let fr = Rect::new(x, y.min(r.y + r.h - 36.0), r.w - 20.0, 28.0);
        if ui.button_ex(fr, if self.follow { "Caméra : suit" } else { "Caméra : libre" }, self.follow, true, pal::ACCENT) {
            acts.push(Act::Follow(!self.follow));
        }
    }

    // ----- HUD --------------------------------------------------------------

    fn top_bar(&self, ui: &mut Ui, acts: &mut Vec<Act>) {
        let sw = screen_width();
        let r = Rect::new(0.0, 0.0, sw, 46.0);
        ui.block(r);
        draw_rectangle(r.x, r.y, r.w, r.h, pal::PANEL);
        draw_line(0.0, r.h, sw, r.h, 1.5, with_alpha(pal::ACCENT, 0.35));
        let me = &self.world.players[self.me as usize];
        let pc = player_color(self.me);
        let mut x = 14.0;
        draw_poly(x + 8.0, 23.0, 6, 9.0, 30.0, pc);
        x += 24.0;
        gfx::text(&me.name, x, 14.0, 16.0, pc);
        x += gfx::text_width(&me.name, 16.0) + 24.0;
        let stat = |x: &mut f32, label: &str, value: String, color: Color| {
            gfx::text(label, *x, 8.0, 11.0, pal::DIM);
            gfx::text(&value, *x, 21.0, 18.0, color);
            *x += gfx::text_width(label, 11.0).max(gfx::text_width(&value, 18.0)) + 26.0;
        };
        stat(&mut x, "MINERAI", format!("{}", me.minerals), pal::ORE);
        stat(&mut x, "DRONES", format!("{}/{}", self.world.drone_count(self.me), MAX_DRONES_PER_PLAYER), pal::TEXT);
        stat(&mut x, "USINES", format!("{}", self.world.factory_count(self.me)), pal::TEXT);
        stat(&mut x, "EXTRAIT", format!("{}", me.mined), pal::DIM);
        for p in &self.world.players {
            if p.id == self.me {
                continue;
            }
            let c = player_color(p.id);
            let v = if p.alive {
                format!("{} usines  {} drones", self.world.factory_count(p.id), self.world.drone_count(p.id))
            } else {
                "éliminé".into()
            };
            stat(&mut x, &p.name.to_uppercase(), v, c);
        }

        // Right side: time, speed, menu.
        let secs = (self.world.tick as f32 / TICKS_PER_SEC) as u32;
        let mut rx = sw - 90.0;
        if ui.button(Rect::new(rx, 8.0, 80.0, 30.0), "Menu") {
            acts.push(Act::Menu);
        }
        if !self.is_net() {
            for (s, label) in [(8.0, "x8"), (4.0, "x4"), (2.0, "x2"), (1.0, "x1"), (0.0, "II")].iter() {
                rx -= 40.0;
                let active = (self.speed - s).abs() < 0.01;
                if ui.button_ex(Rect::new(rx, 8.0, 36.0, 30.0), label, active, true, pal::ACCENT) {
                    acts.push(Act::Speed(*s));
                }
            }
        } else if let Driver::Net { lock, .. } = &self.driver {
            let waiting = lock.waiting(self.world.tick);
            let s = if waiting { "attente du réseau…" } else { "en ligne" };
            rx -= gfx::text_width(s, 14.0) + 20.0;
            gfx::text(s, rx, 15.0, 14.0, if waiting { pal::WARN } else { pal::GOOD });
        }
        let t = format!("{:02}:{:02}  tick {}", secs / 60, secs % 60, self.world.tick);
        rx -= gfx::text_width(&t, 14.0) + 24.0;
        gfx::text(&t, rx, 15.0, 14.0, pal::DIM);
        let title_w = gfx::text_width(&self.title, 13.0);
        if rx - title_w - 20.0 > x {
            gfx::text(&self.title, rx - title_w - 20.0, 16.0, 13.0, pal::FAINT);
        }
    }

    fn toolbar(&self, ui: &mut Ui, acts: &mut Vec<Act>) {
        let y = screen_height() - 54.0;
        let mut x = 10.0;
        let me = &self.world.players[self.me as usize];
        let can = me.minerals >= FACTORY_COST && me.alive;
        let label = format!("Usine {FACTORY_COST}  [B]");
        let w = gfx::text_width(&label, 15.0) + 30.0;
        if ui.button_ex(Rect::new(x, y, w, 44.0), &label, self.building, can || self.building, pal::GOOD) {
            acts.push(Act::ToggleBuild);
        }
        if ui.hover(Rect::new(x, y, w, 44.0)) {
            ui.tooltip(&format!(
                "Construire une usine ({FACTORY_COST} minerai) à moins de {BUILD_RANGE} cases d'une de vos unités"
            ));
        }
        x += w + 8.0;
        let bw = 150.0;
        if ui.button(Rect::new(x, y, bw, 44.0), "Programmes [P]") {
            let name = self.default_program_name();
            acts.push(if self.editor_open { Act::CloseEditor } else { Act::OpenProgram(name, None) });
        }
        x += bw + 8.0;
        if ui.button(Rect::new(x, y, 100.0, 44.0), "Aide [F1]") {
            acts.push(Act::ToggleHelp);
        }
        x += 108.0;
        if ui.button(Rect::new(x, y, 90.0, 44.0), "Base [H]")
            && let Some(f) = self.world.factories.iter().find(|f| f.owner == self.me) {
                acts.push(Act::CenterOn(tile_center(f.x, f.y)));
            }
    }

    fn default_program_name(&self) -> String {
        if let Sel::Factory(id) = self.sel
            && let Some(f) = self.world.factory(id)
                && f.owner == self.me {
                    return f.program.name.clone();
                }
        "mineur".into()
    }

    fn selection_panel(&self, ui: &mut Ui, lib: &Library, acts: &mut Vec<Act>) {
        let w = 330.0;
        let x = screen_width() - w - 10.0;
        let y = 56.0;
        match self.sel {
            Sel::None => {}
            Sel::Factory(id) => {
                let Some(f) = self.world.factory(id) else { return };
                let mine = f.owner == self.me;
                let h = if mine { 300.0 } else { 110.0 };
                let r = Rect::new(x, y, w, h);
                ui.panel(r);
                let pc = player_color(f.owner);
                let owner = &self.world.players[f.owner as usize].name;
                gfx::text(&format!("USINE #{id}"), x + 14.0, y + 12.0, 20.0, pc);
                gfx::text(owner, x + w - 14.0 - gfx::text_width(owner, 14.0), y + 16.0, 14.0, pal::DIM);
                let mut cy = y + 44.0;
                bar(x + 14.0, cy, w - 28.0, "PV", f.hp, FACTORY_HP, mix(pal::BAD, pal::GOOD, f.hp as f32 / FACTORY_HP as f32));
                cy += 30.0;
                gfx::text("Programme", x + 14.0, cy, 13.0, pal::DIM);
                gfx::text(&f.program.name, x + 100.0, cy - 2.0, 17.0, pal::ACCENT);
                if !mine {
                    return;
                }
                cy += 24.0;
                if ui.button_ex(Rect::new(x + 14.0, cy, 140.0, 30.0), "Éditer  [E]", false, true, pal::GOOD) {
                    acts.push(Act::OpenProgram(f.program.name.clone(), Some(id)));
                }
                if ui.button_ex(Rect::new(x + 162.0, cy, 154.0, 30.0), "Changer", self.program_menu == Some(id), true, pal::ACCENT) {
                    acts.push(Act::ToggleProgramMenu(id));
                }
                cy += 42.0;
                let n = self.world.drones_of_factory(id);
                gfx::text("Drones", x + 14.0, cy + 6.0, 13.0, pal::DIM);
                gfx::text(&format!("{n} / {}", f.quota), x + 100.0, cy + 3.0, 18.0, pal::TEXT);
                if ui.button(Rect::new(x + 230.0, cy, 40.0, 28.0), "-") && f.quota > 0 {
                    acts.push(Act::Issue(Command::SetQuota { factory: id, quota: f.quota - 1 }));
                }
                if ui.button(Rect::new(x + 276.0, cy, 40.0, 28.0), "+") && f.quota < MAX_QUOTA {
                    acts.push(Act::Issue(Command::SetQuota { factory: id, quota: f.quota + 1 }));
                }
                cy += 38.0;
                let (label, col) = if f.enabled { ("Production : active", pal::GOOD) } else { ("Production : en pause", pal::WARN) };
                if ui.button_ex(Rect::new(x + 14.0, cy, w - 28.0, 30.0), label, false, true, col) {
                    acts.push(Act::Issue(Command::ToggleFactory { factory: id }));
                }
                cy += 42.0;
                gfx::text("MÉMOIRE DE RUCHE", x + 14.0, cy, 11.0, pal::DIM);
                cy += 16.0;
                let cw = (w - 28.0) / 8.0;
                for i in 0..HIVE_MEM {
                    let cx = x + 14.0 + (i % 8) as f32 * cw;
                    let yy = cy + (i / 8) as f32 * 22.0;
                    let v = f.mem[i];
                    draw_rectangle_lines(cx, yy, cw - 3.0, 19.0, 1.0, if v != 0 { pal::ACCENT } else { pal::FAINT });
                    gfx::text(&format!("{v}"), cx + 3.0, yy + 3.0, 12.0, if v != 0 { pal::TEXT } else { pal::FAINT });
                }

                // Program picker.
                if self.program_menu == Some(id) {
                    let n = lib.entries.len();
                    let pr = Rect::new(x - 190.0, y + 98.0, 180.0, 12.0 + n as f32 * 28.0);
                    ui.panel(pr);
                    for (i, e) in lib.entries.iter().enumerate() {
                        let rr = Rect::new(pr.x + 6.0, pr.y + 6.0 + i as f32 * 28.0, pr.w - 12.0, 26.0);
                        let current = e.name == f.program.name;
                        if ui.button_ex(rr, &e.name, current, true, pal::ACCENT) {
                            match vm::assemble(&e.name, &e.source) {
                                Ok(_) => acts.push(Act::Issue(Command::SetProgram {
                                    factory: id,
                                    name: e.name.clone(),
                                    source: e.source.clone(),
                                })),
                                Err(_) => acts.push(Act::OpenProgram(e.name.clone(), Some(id))),
                            }
                            acts.push(Act::ToggleProgramMenu(id));
                        }
                        if ui.hover(rr)
                            && let Some(b) = programs::builtin(&e.name) {
                                ui.tooltip(b.summary);
                            }
                    }
                }
            }
            Sel::Drone(id) => {
                let Some(d) = self.world.drone(id) else { return };
                let mine = d.owner == self.me;
                let r = Rect::new(x, y, w, if mine { 232.0 } else { 120.0 });
                ui.panel(r);
                let pc = player_color(d.owner);
                gfx::text(&format!("DRONE #{id}"), x + 14.0, y + 12.0, 20.0, pc);
                let owner = &self.world.players[d.owner as usize].name;
                gfx::text(owner, x + w - 14.0 - gfx::text_width(owner, 14.0), y + 16.0, 14.0, pal::DIM);
                let mut cy = y + 44.0;
                bar(x + 14.0, cy, w - 28.0, "PV", d.hp, DRONE_HP, mix(pal::BAD, pal::GOOD, d.hp as f32 / DRONE_HP as f32));
                cy += 24.0;
                bar(x + 14.0, cy, w - 28.0, "Soute", d.cargo, CARGO_CAP, pal::ORE);
                cy += 30.0;
                gfx::text("Programme", x + 14.0, cy, 13.0, pal::DIM);
                gfx::text(&d.program.name, x + 100.0, cy - 2.0, 17.0, pal::ACCENT);
                if !mine {
                    return;
                }
                cy += 26.0;
                let mut rx = x + 14.0;
                for reg in Reg::ALL.iter().chain([Reg::Ok].iter()) {
                    let s = format!("{}={}", reg.name(), d.vm.reg(*reg));
                    gfx::text(&s, rx, cy, 14.0, token_color(vm::TokKind::Reg));
                    rx += gfx::text_width(&s, 14.0) + 12.0;
                }
                cy += 26.0;
                if let Some(line) = d.program.line_of(d.vm.last_pc) {
                    let src = d.program.source.lines().nth(line).unwrap_or("").trim();
                    let s = format!("{:>3}  {}", line + 1, vm::strip_comment(src).trim());
                    gfx::text(&s, x + 14.0, cy, 14.0, pal::TEXT);
                }
                cy += 26.0;
                if ui.button_ex(Rect::new(x + 14.0, cy, 150.0, 30.0), "Déboguer  [E]", false, true, pal::GOOD) {
                    acts.push(Act::OpenDebug(id));
                }
                if ui.button_ex(Rect::new(x + 172.0, cy, 144.0, 30.0), "Suivre", self.follow, true, pal::ACCENT) {
                    acts.push(Act::Follow(!self.follow));
                }
            }
        }
    }

    fn draw_toasts(&self) {
        let mut y = screen_height() - 74.0;
        for t in self.toasts.iter().rev() {
            let a = (1.0 - ((t.t - 3.5) / 1.0).max(0.0)).clamp(0.0, 1.0) * (t.t * 6.0).min(1.0);
            let w = gfx::text_width(&t.text, 15.0) + 24.0;
            let r = Rect::new(10.0, y - 30.0, w, 28.0);
            draw_rectangle(r.x, r.y, r.w, r.h, with_alpha(pal::PANEL, 0.9 * a));
            draw_rectangle(r.x, r.y, 3.0, r.h, with_alpha(t.color, a));
            gfx::text(&t.text, r.x + 12.0, r.y + 6.0, 15.0, with_alpha(mix(t.color, WHITE, 0.5), a));
            y -= 34.0;
        }
    }

    fn overlays(&self, ui: &mut Ui, acts: &mut Vec<Act>) {
        let (sw, sh) = (screen_width(), screen_height());
        if let Driver::Net { lock, lost, .. } = &self.driver {
            if let Some(t) = lock.desync {
                let s = format!("Désynchronisation détectée (tick {t}) : les deux mondes divergent.");
                let r = Rect::new(sw / 2.0 - 300.0, 56.0, 600.0, 30.0);
                draw_rectangle(r.x, r.y, r.w, r.h, with_alpha(pal::BAD, 0.25));
                gfx::text_centered(&s, sw / 2.0, r.y + 7.0, 15.0, pal::BAD);
            }
            if let Some(msg) = lost {
                let r = Rect::new(sw / 2.0 - 260.0, sh / 2.0 - 80.0, 520.0, 160.0);
                ui.panel(r);
                gfx::text_centered("PARTIE INTERROMPUE", sw / 2.0, r.y + 22.0, 24.0, pal::WARN);
                gfx::text_centered(msg, sw / 2.0, r.y + 66.0, 15.0, pal::TEXT);
                if ui.button(Rect::new(sw / 2.0 - 80.0, r.y + 104.0, 160.0, 36.0), "Menu") {
                    acts.push(Act::Menu);
                }
                return;
            }
        }
        let Some(winner) = self.world.winner() else { return };
        if self.over_dismissed {
            return;
        }
        let won = winner == self.me;
        let r = Rect::new(sw / 2.0 - 240.0, sh / 2.0 - 140.0, 480.0, 280.0);
        ui.panel(r);
        let (title, col) = if won { ("VICTOIRE", pal::GOOD) } else { ("DÉFAITE", pal::BAD) };
        gfx::text_centered(title, sw / 2.0, r.y + 24.0, 40.0, col);
        let me = &self.world.players[self.me as usize];
        let secs = (self.world.tick as f32 / TICKS_PER_SEC) as u32;
        let lines = [
            format!("Durée : {:02}:{:02}", secs / 60, secs % 60),
            format!("Minerai extrait : {}", me.mined),
            format!("Drones construits : {}", me.drones_built),
            format!("Drones ennemis détruits : {}", me.kills),
        ];
        for (i, l) in lines.iter().enumerate() {
            gfx::text_centered(l, sw / 2.0, r.y + 90.0 + i as f32 * 26.0, 16.0, pal::TEXT);
        }
        if ui.button(Rect::new(sw / 2.0 - 190.0, r.y + 220.0, 180.0, 38.0), "Continuer à regarder") {
            acts.push(Act::DismissOver);
        }
        if ui.button(Rect::new(sw / 2.0 + 10.0, r.y + 220.0, 180.0, 38.0), "Menu") {
            acts.push(Act::Menu);
        }
    }

    // ----- input ------------------------------------------------------------

    fn world_input(&mut self, ui: &mut Ui, lib: &Library, acts: &mut Vec<Act>) {
        let over = ui.over_ui();
        let dt = get_frame_time().min(0.1);

        // Zoom towards the mouse.
        if !over && ui.wheel != 0.0 {
            let before = self.cam.s2w(ui.mouse);
            self.cam.zoom = (self.cam.zoom * 1.15f32.powf(ui.wheel.signum())).clamp(8.0, 72.0);
            let after = self.cam.s2w(ui.mouse);
            self.cam.center += before - after;
        }

        // Drag to pan (right or middle button).
        let drag_down = is_mouse_button_down(MouseButton::Right) || is_mouse_button_down(MouseButton::Middle);
        if (ui.right_pressed || is_mouse_button_pressed(MouseButton::Middle)) && !over {
            self.right_drag = Some((ui.mouse, ui.mouse));
        }
        if let Some((start, last)) = self.right_drag {
            if drag_down {
                let d = ui.mouse - last;
                self.cam.center -= d / self.cam.zoom;
                self.right_drag = Some((start, ui.mouse));
                if d != Vec2::ZERO {
                    self.follow = false;
                }
            } else {
                self.right_drag = None;
                // A right click that did not move: cancel / deselect.
                if (ui.mouse - start).length() < 5.0 {
                    if self.building {
                        self.building = false;
                    } else {
                        acts.push(Act::Select(Sel::None));
                    }
                }
            }
        }

        // Keyboard pan.
        if !ui.keyboard_taken {
            let mut v = Vec2::ZERO;
            if is_key_down(KeyCode::W) || is_key_down(KeyCode::Z) || is_key_down(KeyCode::Up) {
                v.y -= 1.0;
            }
            if is_key_down(KeyCode::S) || is_key_down(KeyCode::Down) {
                v.y += 1.0;
            }
            if is_key_down(KeyCode::A) || is_key_down(KeyCode::Q) || is_key_down(KeyCode::Left) {
                v.x -= 1.0;
            }
            if is_key_down(KeyCode::D) || is_key_down(KeyCode::Right) {
                v.x += 1.0;
            }
            if v != Vec2::ZERO && !ui.ctrl {
                self.cam.center += v * dt * 900.0 / self.cam.zoom;
                self.follow = false;
            }
        }
        self.cam.center = self.cam.center.clamp(Vec2::ZERO, vec2(self.world.w as f32, self.world.h as f32));

        if !ui.pressed || over {
            return;
        }
        ui.pressed = false;
        let (tx, ty) = self.cam.tile_at(ui.mouse);
        if self.building {
            match self.world.can_place_factory(self.me, tx, ty) {
                Ok(()) => {
                    let name = self.default_program_name();
                    let source = lib.source_or_builtin(&name).unwrap_or_else(|| programs::TEMPLATE.into());
                    acts.push(Act::Issue(Command::PlaceFactory { x: tx, y: ty, name, source }));
                    if !ui.shift {
                        self.building = false;
                    }
                }
                Err(why) => self.toast(format!("Impossible ici : {why}"), pal::BAD),
            }
            return;
        }
        // Pick the drone closest to the cursor, else whatever is on the tile.
        let wp = self.cam.s2w(ui.mouse);
        let alpha = self.alpha();
        let best = self
            .world
            .drones
            .iter()
            .filter_map(|d| WorldView::drone_pos(&self.world, d.id, alpha).map(|p| (d.id, p.distance(wp))))
            .filter(|(_, dist)| *dist < 0.55)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        let sel = match (best, self.world.cell(tx, ty)) {
            (Some((id, _)), _) => Sel::Drone(id),
            (None, Cell::Factory(id, _)) => Sel::Factory(id),
            (None, Cell::Drone(id, _)) => Sel::Drone(id),
            _ => Sel::None,
        };
        if ui.double_click && sel == self.sel
            && let Sel::Factory(id) = sel
                && let Some(f) = self.world.factory(id).filter(|f| f.owner == self.me) {
                    acts.push(Act::OpenProgram(f.program.name.clone(), Some(id)));
                }
        acts.push(Act::Select(sel));
    }

    fn hotkeys(&mut self, ui: &mut Ui, acts: &mut Vec<Act>) {
        if is_key_pressed(KeyCode::F1) {
            acts.push(Act::ToggleHelp);
        }
        if is_key_pressed(KeyCode::Escape) {
            if self.help_open {
                acts.push(Act::ToggleHelp);
            } else if self.editor.focused || self.edit_name.focused {
                self.editor.focused = false;
                self.edit_name.focused = false;
            } else if self.building {
                self.building = false;
            } else if let Some(f) = self.program_menu {
                acts.push(Act::ToggleProgramMenu(f));
            } else if self.editor_open {
                acts.push(Act::CloseEditor);
            } else {
                acts.push(Act::Select(Sel::None));
            }
        }
        if ui.keyboard_taken {
            return;
        }
        if is_key_pressed(KeyCode::Space) {
            acts.push(Act::Speed(if self.speed > 0.0 { 0.0 } else { 1.0 }));
        }
        for (k, s) in [(KeyCode::Key1, 1.0), (KeyCode::Key2, 2.0), (KeyCode::Key3, 4.0), (KeyCode::Key4, 8.0)] {
            if is_key_pressed(k) {
                acts.push(Act::Speed(s));
            }
        }
        if is_key_pressed(KeyCode::N) {
            acts.push(Act::Step);
        }
        if is_key_pressed(KeyCode::B) {
            acts.push(Act::ToggleBuild);
        }
        if is_key_pressed(KeyCode::P) {
            let name = self.default_program_name();
            acts.push(if self.editor_open { Act::CloseEditor } else { Act::OpenProgram(name, None) });
        }
        if is_key_pressed(KeyCode::E) {
            match self.sel {
                Sel::Factory(id) => {
                    if let Some(f) = self.world.factory(id).filter(|f| f.owner == self.me) {
                        acts.push(Act::OpenProgram(f.program.name.clone(), Some(id)));
                    }
                }
                Sel::Drone(id) => {
                    if self.world.drone(id).is_some_and(|d| d.owner == self.me) {
                        acts.push(Act::OpenDebug(id));
                    }
                }
                Sel::None => {}
            }
        }
        if (is_key_pressed(KeyCode::H) || is_key_pressed(KeyCode::Home))
            && let Some(f) = self.world.factories.iter().find(|f| f.owner == self.me) {
                acts.push(Act::CenterOn(tile_center(f.x, f.y)));
            }
        if is_key_pressed(KeyCode::Tab) {
            let mine: Vec<u32> = self.world.factories.iter().filter(|f| f.owner == self.me).map(|f| f.id).collect();
            if !mine.is_empty() {
                let next = match self.sel {
                    Sel::Factory(id) => mine.iter().position(|f| *f == id).map(|i| (i + 1) % mine.len()).unwrap_or(0),
                    _ => 0,
                };
                let id = mine[next];
                let f = self.world.factory(id).unwrap();
                acts.push(Act::Select(Sel::Factory(id)));
                acts.push(Act::CenterOn(tile_center(f.x, f.y)));
            }
        }
    }
}

fn describe_value(v: i32) -> String {
    let dir = match v {
        0 => " N",
        1 => " E",
        2 => " S",
        3 => " W",
        -1 => " NONE",
        _ => "",
    };
    format!("{v}{}", if dir.is_empty() { String::new() } else { format!("  ({})", dir.trim()) })
}

fn bar(x: f32, y: f32, w: f32, label: &str, v: i32, max: i32, color: Color) {
    gfx::text(label, x, y, 13.0, pal::DIM);
    let bx = x + 86.0;
    let bw = w - 86.0 - 60.0;
    draw_rectangle(bx, y + 2.0, bw, 10.0, with_alpha(BLACK, 0.6));
    draw_rectangle(bx, y + 2.0, bw * (v.max(0) as f32 / max as f32).min(1.0), 10.0, color);
    draw_rectangle_lines(bx, y + 2.0, bw, 10.0, 1.0, with_alpha(color, 0.5));
    gfx::text(&format!("{v}/{max}"), bx + bw + 8.0, y - 1.0, 14.0, pal::TEXT);
}

/// Full language reference.
fn help_overlay(ui: &mut Ui, acts: &mut Vec<Act>) {
    let (sw, sh) = (screen_width(), screen_height());
    let w = (sw - 80.0).min(1240.0);
    let h = (sh - 80.0).min(820.0);
    let r = Rect::new((sw - w) / 2.0, (sh - h) / 2.0, w, h);
    ui.panel(r);
    if ui.button(Rect::new(r.x + r.w - 44.0, r.y + 10.0, 34.0, 30.0), "X") {
        acts.push(Act::ToggleHelp);
    }
    gfx::text("DASM  —  le langage des drones", r.x + 20.0, r.y + 16.0, 22.0, pal::ACCENT);
    gfx::text(
        "Chaque tick, un drone exécute ses instructions jusqu'à une ACTION, qui termine son tour.",
        r.x + 20.0,
        r.y + 46.0,
        14.0,
        pal::DIM,
    );

    let col_w = (w - 60.0) / 2.0;
    let mut y = r.y + 76.0;
    let x = r.x + 20.0;
    // Shrink everything if the window is short.
    let needed = 4.0 * 24.0 + vm::MNEMONICS.len() as f32 * 18.5;
    let k = ((h - 90.0) / needed).clamp(0.62, 1.0);
    let fs = 14.0 * k;
    let lh = 18.5 * k;
    for (cat, title) in [
        (Cat::Action, "ACTIONS  (terminent le tour, mettent ok à 1 ou 0)"),
        (Cat::Sense, "CAPTEURS"),
        (Cat::Data, "DONNÉES"),
        (Cat::Flow, "SAUTS"),
    ] {
        gfx::text(title, x, y, 12.0 * k, token_color(vm::TokKind::Op(cat)));
        y += 18.0 * k;
        for m in vm::MNEMONICS.iter().filter(|m| m.cat == cat) {
            gfx::text(m.syntax, x + 8.0, y, fs, token_color(vm::TokKind::Op(cat)));
            gfx::text(m.help, x + 150.0 * k, y, fs, pal::TEXT);
            y += lh;
        }
        y += 6.0 * k;
    }

    let x2 = r.x + 40.0 + col_w;
    let mut y = r.y + 76.0;
    let section = |y: &mut f32, title: &str, lines: &[&str]| {
        gfx::text(title, x2, *y, 12.0 * k, pal::VIOLET);
        *y += 18.0 * k;
        for l in lines {
            gfx::text(l, x2 + 8.0, *y, fs, pal::TEXT);
            *y += lh;
        }
        *y += 6.0 * k;
    };
    section(&mut y, "REGISTRES", &[
        "a b c d   registres libres du drone",
        "ok        1 si la dernière action a réussi, sinon 0",
        "Les valeurs : registre, nombre (-3, 42) ou constante",
    ]);
    section(&mut y, "DIRECTIONS", &["N=0  E=1  S=2  W=3  NONE=-1   (y augmente vers le sud)"]);
    section(&mut y, "CHOSES  (LOOK, SCAN, DIST, DX, DY, COUNT)", &[
        "EMPTY=0  WALL=1  ORE=2  ALLY=3  ENEMY=4  BASE=5  EBASE=6",
        "BASE / EBASE = usine alliée / ennemie : détectées partout",
        &format!("autres choses : portée de {SENSOR_RANGE} cases (distance de Manhattan)"),
        &format!("FULL = {CARGO_CAP} (capacité de la soute)"),
    ]);
    section(&mut y, "RÈGLES", &[
        &format!("drone {DRONE_COST} minerai, usine {FACTORY_COST}, quota {DEFAULT_QUOTA} drones/usine (réglable)"),
        &format!("au plus {MAX_OPS_PER_TICK} instructions par tick, sinon le drone « réfléchit »"),
        "fin du programme = retour au début ; RET sans CALL aussi",
        &format!("mémoire de ruche : {HIVE_MEM} cases partagées par les drones d'une usine"),
        "SCAN donne une direction naïve : à vous de contourner les murs !",
    ]);
    section(&mut y, "SYNTAXE", &[
        "etiquette:      ; commentaire (aussi # ou //)",
        "    MOVE N      ; virgules facultatives : JEQ a b fin",
    ]);
    section(&mut y, "RACCOURCIS", &[
        "Espace pause  1-4 vitesse  N tick suivant  B usine",
        "E éditer/déboguer  P programmes  H base  Tab usines",
        "ZQSD/WASD/flèches + clic droit glissé : caméra, molette : zoom",
        "Éditeur : Ctrl+S appliquer, Ctrl+Z/Y, Ctrl+D dupliquer, Tab",
    ]);
}

/// Helpers used by command-line switches (screenshots, quick testing).
impl Game {
    pub fn fast_forward(&mut self, ticks: u64) {
        if let Driver::Solo { bot, pending } = &mut self.driver {
            for _ in 0..ticks {
                let mut cmds: Vec<(u8, Command)> = pending.drain(..).map(|c| (self.me, c)).collect();
                if let Some(b) = bot {
                    let p = b.player;
                    cmds.extend(b.think(&self.world).into_iter().map(|c| (p, c)));
                }
                self.world.step(&cmds);
            }
        }
    }

    pub fn open_program_by_name(&mut self, lib: &Library, name: &str) {
        let fid = self.world.factories.iter().find(|f| f.owner == self.me).map(|f| f.id);
        self.sel = fid.map(Sel::Factory).unwrap_or(Sel::None);
        self.open_program(lib, name, fid);
    }

    pub fn set_overlays(&mut self, help: bool, build: bool) {
        self.help_open = help;
        self.building = build;
    }

    pub fn debug_first_drone(&mut self) {
        if let Some(id) = self.world.drones.iter().find(|d| d.owner == self.me).map(|d| d.id) {
            self.open_debug(id);
        }
    }
}
