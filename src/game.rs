//! One running world: drives the simulation (solo, or as a client of a
//! world server), handles input and draws the HUD, panels, program editor
//! and debugger.

use std::collections::VecDeque;

use macroquad::prelude::*;

use game_of_drones::ai::{Bot, Level};
use game_of_drones::net::{ClientMsg, Conn, NetEvent, ServerMsg};
use game_of_drones::programs;
use game_of_drones::rules::*;
use game_of_drones::server::{bot_name, ServerHandle, HASH_EVERY};
use game_of_drones::sim::{Command, Doing, Event, World};
use game_of_drones::vm::{self, Cat, Reg};

use crate::editor::{token_color, Editor};
use crate::gfx::{self, mix, pal, player_color, with_alpha, Bloom};
use crate::library::{self, Library};
use crate::ui::{TextField, Ui};
use crate::view::{draw_minimap, fp, fpv, Cam, Highlight, WorldView};

pub enum Driver {
    Solo {
        bots: Vec<Bot>,
        pending: Vec<Command>,
    },
    Online {
        conn: Conn<ServerMsg>,
        frames: VecDeque<(u64, Vec<(u8, Command)>)>,
        lost: Option<String>,
        world_name: String,
        /// Set when this game also hosts the world: dropping it stops the server.
        _server: Option<ServerHandle>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sel {
    None,
    Factory(u32),
    Drone(u32),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tool {
    None,
    Build,
    Target(u32),
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
    Tool(Tool),
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
    Respawn,
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
    tool: Tool,
    editor_open: bool,
    editor: Editor,
    edit_name: TextField,
    edit_target: EditTarget,
    edit_factory: Option<u32>,
    debug_prog: u32,
    help_open: bool,
    program_menu: Option<u32>,
    toasts: Vec<Toast>,
    follow: bool,
    over_dismissed: bool,
    step_once: bool,
    right_drag: Option<(Vec2, Vec2)>,
    title: String,
}

pub fn player_name() -> String {
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
        let home = world.players.get(me as usize).map(|p| fpv(p.home.0, p.home.1)).unwrap_or(Vec2::splat(fp(world.size) / 2.0));
        let start = world.factories.iter().find(|f| f.owner == me).map(|f| f.id);
        let mut g = Game {
            world,
            me,
            driver,
            cam: Cam { center: home, zoom: 26.0, shake: Vec2::ZERO },
            speed: 1.0,
            acc: 0.0,
            view: WorldView::default(),
            sel: start.map(Sel::Factory).unwrap_or(Sel::None),
            tool: Tool::None,
            editor_open: false,
            editor: Editor::new(),
            edit_name: TextField::new("", 24, library::name_char_ok),
            edit_target: EditTarget::Program,
            edit_factory: None,
            debug_prog: u32::MAX,
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
        if let (Some(fid), Some(src)) = (start, lib.source_or_builtin("mineur"))
            && g.world.factory(fid).is_some_and(|f| g.world.programs[f.prog as usize].source != src)
        {
            g.issue(Command::SetProgram { factory: fid, name: "mineur".into(), source: src });
        }
        g.toast("Cliquez sur votre usine, puis « Éditer » pour programmer vos drones.", pal::ACCENT);
        g
    }

    pub fn solo(lib: &Library, seed: u64, ais: usize, level: Level) -> Game {
        let mut world = World::new(seed);
        let me = world.join(&player_name(), false).unwrap_or(0);
        let bots = (0..ais).filter_map(|i| world.join(&bot_name(i), true)).map(|id| Bot::new(id, level)).collect();
        let title = if ais == 0 { "Bac à sable".to_string() } else { format!("Solo · {ais} IA ({})", level.label()) };
        Game::new(world, me, Driver::Solo { bots, pending: vec![] }, lib, title)
    }

    pub fn online(lib: &Library, conn: Conn<ServerMsg>, me: u8, world: World, world_name: String, server: Option<ServerHandle>) -> Game {
        let title = format!("Monde « {world_name} »");
        let driver = Driver::Online { conn, frames: VecDeque::new(), lost: None, world_name, _server: server };
        Game::new(world, me, driver, lib, title)
    }

    fn is_online(&self) -> bool {
        matches!(self.driver, Driver::Online { .. })
    }

    fn issue(&mut self, cmd: Command) {
        match &mut self.driver {
            Driver::Solo { pending, .. } => pending.push(cmd),
            Driver::Online { conn, .. } => conn.send(&ClientMsg::Cmd(cmd)),
        }
    }

    fn toast(&mut self, text: impl Into<String>, color: Color) {
        self.toasts.push(Toast { text: text.into(), color, t: 0.0 });
        if self.toasts.len() > 5 {
            self.toasts.remove(0);
        }
    }

    pub fn leave(&mut self) {
        if let Driver::Online { conn, .. } = &mut self.driver {
            conn.send(&ClientMsg::Bye);
        }
    }

    fn my_name(&self) -> String {
        self.world.players.get(self.me as usize).map(|p| p.name.clone()).unwrap_or_default()
    }

    // ----- simulation ------------------------------------------------------

    fn advance(&mut self, dt: f32) {
        let mut notes: Vec<Event> = vec![];
        let keep = |e: &Event| matches!(e, Event::PlayerJoined { .. } | Event::PlayerEliminated { .. } | Event::FactoryDestroyed { .. });
        match &mut self.driver {
            Driver::Solo { bots, pending } => {
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
                    for b in bots.iter_mut() {
                        let p = b.player;
                        cmds.extend(b.think(&self.world).into_iter().map(|c| (p, c)));
                    }
                    self.world.step(&cmds);
                    self.view.on_events(&self.world, self.speed);
                    notes.extend(self.world.events.iter().filter(|e| keep(e)).cloned());
                    self.acc -= 1.0;
                    n += 1;
                    if n >= 40 {
                        self.acc = self.acc.min(0.99);
                        break;
                    }
                }
                if self.speed <= 0.0 {
                    self.acc = 0.999;
                }
            }
            Driver::Online { conn, frames, lost, .. } => {
                for ev in conn.poll() {
                    match ev {
                        NetEvent::Msg(ServerMsg::Frame { tick, cmds }) => frames.push_back((tick, cmds)),
                        NetEvent::Msg(ServerMsg::Resync { snapshot }) => match World::from_bytes(&snapshot) {
                            Ok(w) => self.world = w,
                            Err(e) => *lost = Some(format!("monde illisible : {e}")),
                        },
                        NetEvent::Msg(ServerMsg::Refused(m)) => *lost = Some(m),
                        NetEvent::Msg(ServerMsg::Welcome { .. }) => {}
                        NetEvent::Closed(e) => {
                            if lost.is_none() {
                                *lost = Some(format!("Connexion perdue : {e}"));
                            }
                        }
                    }
                }
                while frames.front().is_some_and(|f| f.0 < self.world.tick) {
                    frames.pop_front();
                }
                self.acc += dt * TICKS_PER_SEC;
                let max_steps = if frames.len() > 8 { 25 } else { 3 };
                for _ in 0..max_steps {
                    // Catch up when frames pile up, otherwise follow real time.
                    if self.acc < 1.0 && frames.len() <= 2 {
                        break;
                    }
                    let Some((tick, cmds)) = frames.pop_front() else {
                        self.acc = self.acc.min(1.0);
                        break;
                    };
                    if tick != self.world.tick {
                        frames.push_front((tick, cmds));
                        break;
                    }
                    self.world.step(&cmds);
                    self.view.on_events(&self.world, 1.0);
                    notes.extend(self.world.events.iter().filter(|e| keep(e)).cloned());
                    if self.world.tick.is_multiple_of(HASH_EVERY) {
                        conn.send(&ClientMsg::Hash { tick: self.world.tick, hash: self.world.state_hash() });
                    }
                    self.acc = (self.acc - 1.0).max(0.0);
                }
            }
        }
        for e in notes {
            match e {
                Event::PlayerJoined { player } if player != self.me => {
                    let name = self.world.players[player as usize].name.clone();
                    self.toast(format!("{name} entre dans le monde."), player_color(player));
                }
                Event::PlayerEliminated { player } => {
                    let name = self.world.players[player as usize].name.clone();
                    let col = if player == self.me { pal::BAD } else { player_color(player) };
                    self.toast(format!("{name} a été éliminé."), col);
                }
                Event::FactoryDestroyed { owner, .. } if owner == self.me => {
                    self.toast("Une de vos usines a été détruite !", pal::BAD);
                }
                _ => {}
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
        self.toasts.retain(|t| t.t < 5.0);
        self.validate_selection();

        if self.follow
            && let Sel::Drone(id) = self.sel
            && let Some(p) = WorldView::drone_pos(&self.world, id, self.alpha())
        {
            self.cam.center = self.cam.center.lerp(p, (dt * 5.0).min(1.0));
        }
        self.cam.shake = self.view.shake_offset();

        bloom.begin();
        let hl = self.highlight(ui);
        self.view.draw(&self.world, &self.cam, self.alpha(), &hl);
        bloom.finish();

        let mut acts = vec![];
        self.top_bar(ui, &mut acts);
        self.scoreboard(ui);
        self.toolbar(ui, &mut acts);
        if self.editor_open {
            self.editor_window(ui, lib, &mut acts);
        } else {
            self.selection_panel(ui, lib, &mut acts);
            let mm = Rect::new(screen_width() - 210.0, screen_height() - 210.0, 200.0, 200.0);
            ui.block(mm);
            let me = self.me;
            if let Some(p) = draw_minimap(&self.world, &self.cam, mm, me, ui.mouse, ui.pressed || (ui.down && ui.hover(mm))) {
                ui.pressed = false;
                acts.push(Act::CenterOn(p));
            }
        }
        self.draw_toasts();
        self.tool_hint();
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
            && self.sel != Sel::Factory(fid)
        {
            self.program_menu = None;
        }
        if let Tool::Target(fid) = self.tool
            && self.world.factory(fid).is_none()
        {
            self.tool = Tool::None;
        }
    }

    fn mouse_fp(&self, ui: &Ui) -> (i32, i32) {
        let w = self.cam.s2w(ui.mouse);
        ((w.x * FP as f32) as i32, (w.y * FP as f32) as i32)
    }

    fn highlight(&self, ui: &Ui) -> Highlight {
        let ghost = if self.tool == Tool::Build && !ui.over_ui() {
            let (x, y) = self.mouse_fp(ui);
            Some((fpv(x, y), self.world.can_place_factory(self.me, x, y).is_ok()))
        } else {
            None
        };
        let target = match (self.tool, self.sel) {
            (Tool::Target(_), _) if !ui.over_ui() => Some(self.cam.s2w(ui.mouse)),
            (_, Sel::Factory(id)) => self
                .world
                .factory(id)
                .filter(|f| f.owner == self.me && f.mem[2] == 1)
                .map(|f| vec2(f.mem[0] as f32 + 0.5, f.mem[1] as f32 + 0.5)),
            _ => None,
        };
        Highlight {
            factory: if let Sel::Factory(id) = self.sel { Some(id) } else { None },
            drone: if let Sel::Drone(id) = self.sel { Some(id) } else { None },
            ghost,
            build_range_for: (self.tool == Tool::Build).then_some(self.me),
            target,
            me: self.me,
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
                if !self.is_online() {
                    self.speed = s;
                }
            }
            Act::Step => {
                if !self.is_online() {
                    self.speed = 0.0;
                    self.step_once = true;
                }
            }
            Act::Tool(t) => {
                self.tool = if self.tool == t { Tool::None } else { t };
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
            Act::Respawn => {
                let name = self.my_name();
                self.issue(Command::Join { name, bot: false });
                self.toast("Retour dans le monde…", pal::ACCENT);
            }
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
            .or_else(|| factory.and_then(|f| self.world.factory(f)).map(|f| self.world.programs[f.prog as usize].source.clone()))
            .unwrap_or_else(|| programs::TEMPLATE.to_string());
        self.editor.set_text(&src);
        self.editor.read_only = false;
        self.editor.exec_line = None;
        self.edit_name.text = name.to_string();
        self.edit_target = EditTarget::Program;
        self.edit_factory = factory;
        self.editor_open = true;
        self.editor.focused = true;
        self.tool = Tool::None;
    }

    fn open_debug(&mut self, id: u32) {
        let Some(d) = self.world.drone(id) else { return };
        let (prog, factory) = (d.prog, d.factory);
        self.editor.set_text(&self.world.programs[prog as usize].source);
        self.editor.read_only = true;
        self.edit_name.text = self.world.program_name(prog).to_string();
        self.debug_prog = prog;
        self.edit_target = EditTarget::Debug(id);
        self.edit_factory = Some(factory);
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
                    && f.owner == self.me
                    && self.world.program_name(f.prog) != name
                {
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

    fn users_of(&self, name: &str) -> usize {
        self.world.factories.iter().filter(|f| f.owner == self.me && self.world.program_name(f.prog) == name).count()
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
        let side_w = if debug { 210.0 } else { 0.0 };
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

        if let Some(id) = debug_id {
            match self.world.drone(id) {
                Some(d) => {
                    if d.prog != self.debug_prog {
                        self.debug_prog = d.prog;
                        let scroll = self.editor.scroll;
                        self.editor.set_text(&self.world.programs[d.prog as usize].source);
                        self.editor.scroll = scroll;
                        self.edit_name.text = self.world.program_name(d.prog).to_string();
                    }
                    self.editor.exec_line = self.world.program(d.prog).line_of(d.vm.last_pc);
                }
                None => self.editor.exec_line = None,
            }
        }

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
            if !self.is_online() {
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
                let used = self.users_of(&e.name);
                if current {
                    draw_rectangle(r.x, r.y, r.w, r.h, with_alpha(pal::ACCENT, 0.18));
                } else if ui.hover(r) {
                    draw_rectangle(r.x, r.y, r.w, r.h, with_alpha(pal::ACCENT, 0.08));
                }
                gfx::text(&e.name, r.x + 6.0, r.y + 5.0, 14.0, if current { pal::ACCENT } else { pal::TEXT });
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

        if self.editor.update(ui, l.code) {
            acts.push(Act::Apply);
        }
        if self.editor.focused {
            self.edit_name.focused = false;
        }
        if debug_id.is_some() {
            if let Some(line) = self.editor.exec_line {
                self.editor.reveal(l.code, line);
            }
        } else {
            let src = self.editor.text();
            self.editor.errors = match vm::assemble(&self.edit_name.text, &src) {
                Ok(_) => vec![],
                Err(e) => e,
            };
        }
        self.editor.draw(ui, l.code);

        if let Some(id) = debug_id {
            self.debug_side(ui, l.side, id, acts);
        }

        let f = l.footer;
        draw_rectangle(f.x, f.y, f.w, f.h, with_alpha(BLACK, 0.35));
        if debug_id.is_some() {
            gfx::text("La ligne en surbrillance est l'action en cours de ce drone.", f.x + 10.0, f.y + 10.0, 14.0, pal::DIM);
            let hint = if self.is_online() {
                "En ligne, le monde ne se met pas en pause : observez-le vivre."
            } else {
                "Mettez en pause (Espace) et avancez tick par tick (N) pour suivre son raisonnement."
            };
            gfx::text(hint, f.x + 10.0, f.y + 32.0, 14.0, pal::DIM);
        } else if self.editor.errors.is_empty() {
            let n = vm::assemble("x", &self.editor.text()).map(|p| p.code.len()).unwrap_or(0);
            gfx::text(&format!("OK  {n} instructions"), f.x + 10.0, f.y + 10.0, 15.0, pal::GOOD);
            let users = self.users_of(&self.edit_name.text);
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
            gfx::text(&describe_value(v), x + 40.0, y, 18.0, pal::TEXT);
            y += 24.0;
        }
        y += 8.0;
        gfx::text("ÉTAT", x, y, 12.0, pal::DIM);
        y += 20.0;
        let (state, col) = doing_label(d.doing);
        gfx::text(&state, x, y, 14.0, col);
        y += 22.0;
        let rows = [
            ("PV", format!("{}/{}", d.hp, DRONE_HP)),
            ("soute", format!("{}/{}", d.cargo, CARGO_CAP)),
            ("cap", format!("{}°", d.heading)),
            ("position", format!("{}, {}", d.x / FP, d.y / FP)),
            ("instr.", format!("{}", d.vm.last_ops)),
        ];
        for (k, v) in rows {
            gfx::text(k, x, y, 13.0, pal::DIM);
            gfx::text(&v, x + 80.0, y, 13.0, pal::TEXT);
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
        let Some(me) = self.world.players.get(self.me as usize) else { return };
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

        let secs = (self.world.tick as f32 / TICKS_PER_SEC) as u32;
        let mut rx = sw - 90.0;
        if ui.button(Rect::new(rx, 8.0, 80.0, 30.0), "Menu") {
            acts.push(Act::Menu);
        }
        match &self.driver {
            Driver::Solo { .. } => {
                for (s, label) in [(8.0, "x8"), (4.0, "x4"), (2.0, "x2"), (1.0, "x1"), (0.0, "II")] {
                    rx -= 40.0;
                    let active = (self.speed - s).abs() < 0.01;
                    if ui.button_ex(Rect::new(rx, 8.0, 36.0, 30.0), label, active, true, pal::ACCENT) {
                        acts.push(Act::Speed(s));
                    }
                }
            }
            Driver::Online { frames, lost, .. } => {
                let (s, c) = if lost.is_some() {
                    ("hors ligne", pal::BAD)
                } else if frames.len() > 8 {
                    ("rattrapage…", pal::WARN)
                } else {
                    ("en ligne", pal::GOOD)
                };
                rx -= gfx::text_width(s, 14.0) + 20.0;
                gfx::text(s, rx, 15.0, 14.0, c);
            }
        }
        let h = secs / 3600;
        let t = if h > 0 {
            format!("{h}:{:02}:{:02}", (secs / 60) % 60, secs % 60)
        } else {
            format!("{:02}:{:02}", secs / 60, secs % 60)
        };
        rx -= gfx::text_width(&t, 15.0) + 24.0;
        gfx::text(&t, rx, 14.0, 15.0, pal::DIM);
        let title_w = gfx::text_width(&self.title, 13.0);
        if rx - title_w - 20.0 > x {
            gfx::text(&self.title, rx - title_w - 20.0, 16.0, 13.0, pal::FAINT);
        }
    }

    fn scoreboard(&self, ui: &mut Ui) {
        if self.editor_open && screen_width() < 1300.0 {
            return;
        }
        let mut rows: Vec<(u8, usize, usize, bool)> = self
            .world
            .players
            .iter()
            .map(|p| (p.id, self.world.factory_count(p.id), self.world.drone_count(p.id), p.alive))
            .collect();
        rows.sort_by_key(|r| (!r.3, std::cmp::Reverse(r.1 * 20 + r.2), r.0));
        let shown = rows.len().min(10);
        let r = Rect::new(10.0, 56.0, 250.0, 30.0 + shown as f32 * 20.0);
        ui.block(r);
        draw_rectangle(r.x, r.y, r.w, r.h, with_alpha(pal::PANEL, 0.75));
        let online = if let Driver::Online { world_name, .. } = &self.driver {
            format!("MONDE « {} »", world_name.to_uppercase())
        } else {
            "JOUEURS".into()
        };
        gfx::text(&online, r.x + 8.0, r.y + 6.0, 11.0, pal::DIM);
        gfx::text("usines  drones", r.x + r.w - 110.0, r.y + 6.0, 11.0, pal::FAINT);
        for (i, (id, f, d, alive)) in rows.iter().take(shown).enumerate() {
            let y = r.y + 24.0 + i as f32 * 20.0;
            let p = &self.world.players[*id as usize];
            let c = if *alive { player_color(*id) } else { pal::FAINT };
            draw_rectangle(r.x + 8.0, y + 3.0, 10.0, 10.0, c);
            let mut name = p.name.clone();
            if *id == self.me {
                name.push_str("  (vous)");
            }
            let tc = if *id == self.me { pal::TEXT } else if *alive { mix(c, WHITE, 0.4) } else { pal::FAINT };
            gfx::text(&name, r.x + 24.0, y, 14.0, tc);
            if *alive {
                gfx::text(&format!("{f:>2}"), r.x + r.w - 96.0, y, 14.0, pal::TEXT);
                gfx::text(&format!("{d:>3}"), r.x + r.w - 46.0, y, 14.0, pal::TEXT);
            } else {
                gfx::text("éliminé", r.x + r.w - 80.0, y, 13.0, pal::FAINT);
            }
        }
    }

    fn toolbar(&self, ui: &mut Ui, acts: &mut Vec<Act>) {
        let y = screen_height() - 54.0;
        let mut x = 10.0;
        let Some(me) = self.world.players.get(self.me as usize) else { return };
        let can = me.minerals >= FACTORY_COST && me.alive;
        let label = format!("Usine {FACTORY_COST}  [B]");
        let w = gfx::text_width(&label, 15.0) + 30.0;
        let building = self.tool == Tool::Build;
        if ui.button_ex(Rect::new(x, y, w, 44.0), &label, building, can || building, pal::GOOD) {
            acts.push(Act::Tool(Tool::Build));
        }
        if ui.hover(Rect::new(x, y, w, 44.0)) {
            ui.tooltip(&format!(
                "Construire une usine ({FACTORY_COST} minerai) à moins de {} unités d'une de vos unités",
                BUILD_RANGE / FP
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
        if ui.button(Rect::new(x, y, 90.0, 44.0), "Base [H]") {
            acts.push(Act::CenterOn(self.home()));
        }
    }

    fn home(&self) -> Vec2 {
        self.world
            .factories
            .iter()
            .find(|f| f.owner == self.me)
            .map(|f| fpv(f.x, f.y))
            .or_else(|| self.world.players.get(self.me as usize).map(|p| fpv(p.home.0, p.home.1)))
            .unwrap_or(self.cam.center)
    }

    fn default_program_name(&self) -> String {
        if let Sel::Factory(id) = self.sel
            && let Some(f) = self.world.factory(id)
            && f.owner == self.me
        {
            return self.world.program_name(f.prog).to_string();
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
                let h = if mine { 340.0 } else { 110.0 };
                let r = Rect::new(x, y, w, h);
                ui.panel(r);
                let pc = player_color(f.owner);
                let owner = &self.world.players[f.owner as usize].name;
                gfx::text(&format!("USINE #{id}"), x + 14.0, y + 12.0, 20.0, pc);
                gfx::text(owner, x + w - 14.0 - gfx::text_width(owner, 14.0), y + 16.0, 14.0, pal::DIM);
                let mut cy = y + 44.0;
                bar(x + 14.0, cy, w - 28.0, "PV", f.hp, FACTORY_HP, mix(pal::BAD, pal::GOOD, f.hp as f32 / FACTORY_HP as f32));
                cy += 30.0;
                let pname = self.world.program_name(f.prog);
                gfx::text("Programme", x + 14.0, cy, 13.0, pal::DIM);
                gfx::text(pname, x + 100.0, cy - 2.0, 17.0, pal::ACCENT);
                if !mine {
                    return;
                }
                cy += 24.0;
                if ui.button_ex(Rect::new(x + 14.0, cy, 140.0, 30.0), "Éditer  [E]", false, true, pal::GOOD) {
                    acts.push(Act::OpenProgram(pname.to_string(), Some(id)));
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
                cy += 40.0;
                let targeting = self.tool == Tool::Target(id);
                let tl = if f.mem[2] == 1 { format!("Cible : {}, {}", f.mem[0], f.mem[1]) } else { "Pas de cible".into() };
                gfx::text(&tl, x + 14.0, cy + 7.0, 14.0, if f.mem[2] == 1 { pal::TEXT } else { pal::DIM });
                if ui.button_ex(Rect::new(x + 176.0, cy, 90.0, 28.0), "Cible [T]", targeting, true, pal::PINK) {
                    acts.push(Act::Tool(Tool::Target(id)));
                }
                if ui.hover(Rect::new(x + 176.0, cy, 90.0, 28.0)) {
                    ui.tooltip("Pose un point sur la carte : mémoire 0 = X, 1 = Y, 2 = 1 (lue par « chasseur »)");
                }
                if ui.button_ex(Rect::new(x + 272.0, cy, 44.0, 28.0), "X", false, f.mem[2] != 0, pal::PINK) {
                    acts.push(Act::Issue(Command::SetMemory { factory: id, index: 2, value: 0 }));
                }
                cy += 40.0;
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

                if self.program_menu == Some(id) {
                    let n = lib.entries.len();
                    let pr = Rect::new(x - 190.0, y + 98.0, 180.0, 12.0 + n as f32 * 28.0);
                    ui.panel(pr);
                    for (i, e) in lib.entries.iter().enumerate() {
                        let rr = Rect::new(pr.x + 6.0, pr.y + 6.0 + i as f32 * 28.0, pr.w - 12.0, 26.0);
                        if ui.button_ex(rr, &e.name, e.name == pname, true, pal::ACCENT) {
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
                            && let Some(b) = programs::builtin(&e.name)
                        {
                            ui.tooltip(b.summary);
                        }
                    }
                }
            }
            Sel::Drone(id) => {
                let Some(d) = self.world.drone(id) else { return };
                let mine = d.owner == self.me;
                let r = Rect::new(x, y, w, if mine { 258.0 } else { 120.0 });
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
                gfx::text(self.world.program_name(d.prog), x + 100.0, cy - 2.0, 17.0, pal::ACCENT);
                if !mine {
                    return;
                }
                cy += 26.0;
                let (state, col) = doing_label(d.doing);
                gfx::text(&format!("{state}  ·  cap {}°", d.heading), x + 14.0, cy, 14.0, col);
                cy += 24.0;
                let mut rx = x + 14.0;
                for reg in Reg::ALL.iter().chain([Reg::Ok].iter()) {
                    let s = format!("{}={}", reg.name(), d.vm.reg(*reg));
                    gfx::text(&s, rx, cy, 14.0, token_color(vm::TokKind::Reg));
                    rx += gfx::text_width(&s, 14.0) + 12.0;
                }
                cy += 24.0;
                let program = self.world.program(d.prog);
                if let Some(line) = program.line_of(d.vm.last_pc) {
                    let src = program.source.lines().nth(line).unwrap_or("").trim();
                    let s = format!("{:>3}  {}", line + 1, vm::strip_comment(src).trim());
                    gfx::text(&s, x + 14.0, cy, 14.0, pal::TEXT);
                }
                cy += 30.0;
                if ui.button_ex(Rect::new(x + 14.0, cy, 150.0, 30.0), "Déboguer  [E]", false, true, pal::GOOD) {
                    acts.push(Act::OpenDebug(id));
                }
                if ui.button_ex(Rect::new(x + 172.0, cy, 144.0, 30.0), "Suivre", self.follow, true, pal::ACCENT) {
                    acts.push(Act::Follow(!self.follow));
                }
            }
        }
    }

    fn tool_hint(&self) {
        let text = match self.tool {
            Tool::None => return,
            Tool::Build => "Construction : clic gauche pour poser (Maj = plusieurs), clic droit pour annuler.".to_string(),
            Tool::Target(id) => format!("Cible de l'usine #{id} : cliquez sur la carte, clic droit pour annuler."),
        };
        let w = gfx::text_width(&text, 15.0) + 24.0;
        let r = Rect::new(screen_width() / 2.0 - w / 2.0, 54.0, w, 28.0);
        draw_rectangle(r.x, r.y, r.w, r.h, with_alpha(pal::PANEL, 0.9));
        draw_rectangle_lines(r.x, r.y, r.w, r.h, 1.0, with_alpha(pal::GOOD, 0.6));
        gfx::text(&text, r.x + 12.0, r.y + 6.0, 15.0, pal::TEXT);
    }

    fn draw_toasts(&self) {
        let mut y = screen_height() - 74.0;
        for t in self.toasts.iter().rev() {
            let a = (1.0 - ((t.t - 4.0) / 1.0).max(0.0)).clamp(0.0, 1.0) * (t.t * 6.0).min(1.0);
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
        if let Driver::Online { lost: Some(msg), .. } = &self.driver {
            let r = Rect::new(sw / 2.0 - 260.0, sh / 2.0 - 80.0, 520.0, 160.0);
            ui.panel(r);
            gfx::text_centered("DÉCONNECTÉ", sw / 2.0, r.y + 22.0, 24.0, pal::WARN);
            gfx::text_centered(msg, sw / 2.0, r.y + 66.0, 15.0, pal::TEXT);
            if ui.button(Rect::new(sw / 2.0 - 80.0, r.y + 104.0, 160.0, 36.0), "Menu") {
                acts.push(Act::Menu);
            }
            return;
        }
        let me_alive = self.world.players.get(self.me as usize).is_some_and(|p| p.alive);
        if self.is_online() {
            if !me_alive {
                let r = Rect::new(sw / 2.0 - 240.0, sh / 2.0 - 100.0, 480.0, 200.0);
                ui.panel(r);
                gfx::text_centered("ÉLIMINÉ", sw / 2.0, r.y + 22.0, 36.0, pal::BAD);
                gfx::text_centered("Le monde continue sans vous… pour l'instant.", sw / 2.0, r.y + 80.0, 15.0, pal::TEXT);
                if ui.button_ex(Rect::new(sw / 2.0 - 190.0, r.y + 130.0, 180.0, 38.0), "Réapparaître", false, true, pal::GOOD) {
                    acts.push(Act::Respawn);
                }
                if ui.button(Rect::new(sw / 2.0 + 10.0, r.y + 130.0, 180.0, 38.0), "Menu") {
                    acts.push(Act::Menu);
                }
            }
            return;
        }
        let decided = if me_alive { self.world.winner().map(|w| w == self.me) } else { Some(false) };
        let Some(won) = decided else { return };
        if self.over_dismissed || self.world.players.len() < 2 {
            return;
        }
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

        if !over && ui.wheel != 0.0 {
            let before = self.cam.s2w(ui.mouse);
            self.cam.zoom = (self.cam.zoom * 1.15f32.powf(ui.wheel.signum())).clamp(2.0, 70.0);
            let after = self.cam.s2w(ui.mouse);
            self.cam.center += before - after;
        }

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
                if (ui.mouse - start).length() < 5.0 {
                    if self.tool != Tool::None {
                        self.tool = Tool::None;
                    } else {
                        acts.push(Act::Select(Sel::None));
                    }
                }
            }
        }

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
        let size = fp(self.world.size);
        self.cam.center = self.cam.center.clamp(Vec2::ZERO, Vec2::splat(size));

        if !ui.pressed || over {
            return;
        }
        ui.pressed = false;
        let (mx, my) = self.mouse_fp(ui);
        match self.tool {
            Tool::Build => {
                match self.world.can_place_factory(self.me, mx, my) {
                    Ok(()) => {
                        let name = self.default_program_name();
                        let source = lib.source_or_builtin(&name).unwrap_or_else(|| programs::TEMPLATE.into());
                        acts.push(Act::Issue(Command::PlaceFactory { x: mx, y: my, name, source }));
                        if !ui.shift {
                            self.tool = Tool::None;
                        }
                    }
                    Err(why) => self.toast(format!("Impossible ici : {why}"), pal::BAD),
                }
                return;
            }
            Tool::Target(fid) => {
                for (i, v) in [(0u8, mx / FP), (1, my / FP), (2, 1)] {
                    acts.push(Act::Issue(Command::SetMemory { factory: fid, index: i, value: v }));
                }
                self.toast(format!("Cible de l'usine #{fid} : {}, {}", mx / FP, my / FP), pal::PINK);
                self.tool = Tool::None;
                return;
            }
            Tool::None => {}
        }
        let wp = self.cam.s2w(ui.mouse);
        let alpha = self.alpha();
        let pick = (0.9f32).max(10.0 / self.cam.zoom);
        let drone = self
            .world
            .drones
            .iter()
            .filter_map(|d| WorldView::drone_pos(&self.world, d.id, alpha).map(|p| (d.id, p.distance(wp))))
            .filter(|(_, dist)| *dist < pick)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        let factory = self
            .world
            .factories
            .iter()
            .map(|f| (f.id, fpv(f.x, f.y).distance(wp)))
            .filter(|(_, d)| *d < fp(FACTORY_RADIUS) + 0.4)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        let sel = match (drone, factory) {
            (_, Some((id, _))) if drone.is_none_or(|d| d.1 > 0.5) => Sel::Factory(id),
            (Some((id, _)), _) => Sel::Drone(id),
            _ => Sel::None,
        };
        if ui.double_click
            && sel == self.sel
            && let Sel::Factory(id) = sel
            && let Some(f) = self.world.factory(id).filter(|f| f.owner == self.me)
        {
            acts.push(Act::OpenProgram(self.world.program_name(f.prog).to_string(), Some(id)));
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
            } else if self.tool != Tool::None {
                self.tool = Tool::None;
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
            acts.push(Act::Tool(Tool::Build));
        }
        if is_key_pressed(KeyCode::T)
            && let Sel::Factory(id) = self.sel
            && self.world.factory(id).is_some_and(|f| f.owner == self.me)
        {
            acts.push(Act::Tool(Tool::Target(id)));
        }
        if is_key_pressed(KeyCode::P) {
            let name = self.default_program_name();
            acts.push(if self.editor_open { Act::CloseEditor } else { Act::OpenProgram(name, None) });
        }
        if is_key_pressed(KeyCode::E) {
            match self.sel {
                Sel::Factory(id) => {
                    if let Some(f) = self.world.factory(id).filter(|f| f.owner == self.me) {
                        acts.push(Act::OpenProgram(self.world.program_name(f.prog).to_string(), Some(id)));
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
        if is_key_pressed(KeyCode::H) || is_key_pressed(KeyCode::Home) {
            acts.push(Act::CenterOn(self.home()));
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
                acts.push(Act::CenterOn(fpv(f.x, f.y)));
            }
        }
    }
}

/// Helpers used by command-line switches (screenshots, quick testing).
impl Game {
    pub fn fast_forward(&mut self, ticks: u64) {
        if let Driver::Solo { bots, pending } = &mut self.driver {
            for _ in 0..ticks {
                let mut cmds: Vec<(u8, Command)> = pending.drain(..).map(|c| (self.me, c)).collect();
                for b in bots.iter_mut() {
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
        if build {
            self.tool = Tool::Build;
        }
    }

    pub fn debug_first_drone(&mut self) {
        if let Some(id) = self.world.drones.iter().find(|d| d.owner == self.me).map(|d| d.id) {
            self.open_debug(id);
        }
    }

    pub fn set_zoom(&mut self, z: f32) {
        self.cam.zoom = z;
    }
}

fn doing_label(d: Doing) -> (String, Color) {
    match d {
        Doing::Idle => ("réfléchit".into(), pal::TEXT),
        Doing::Move { .. } => ("avance".into(), pal::TEXT),
        Doing::Mine { .. } => ("mine".into(), pal::ORE),
        Doing::Drop { .. } => ("décharge".into(), pal::ORE),
        Doing::Attack { .. } => ("tire".into(), pal::BAD),
        Doing::Wait => ("attend".into(), pal::TEXT),
        Doing::Fail => ("action impossible".into(), pal::BAD),
        Doing::Stall => (format!("calcule (> {MAX_OPS_PER_TICK} instr.)"), pal::WARN),
    }
}

fn describe_value(v: i32) -> String {
    let name = match v {
        0 => "N",
        90 => "E",
        180 => "S",
        270 => "W",
        -1 => "NONE",
        _ => "",
    };
    if name.is_empty() { format!("{v}") } else { format!("{v}  ({name})") }
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
    let w = (sw - 80.0).min(1280.0);
    let h = (sh - 80.0).min(840.0);
    let r = Rect::new((sw - w) / 2.0, (sh - h) / 2.0, w, h);
    ui.panel(r);
    if ui.button(Rect::new(r.x + r.w - 44.0, r.y + 10.0, 34.0, 30.0), "X") {
        acts.push(Act::ToggleHelp);
    }
    gfx::text("DASM  —  le langage des drones", r.x + 20.0, r.y + 16.0, 22.0, pal::ACCENT);
    gfx::text(
        "Un drone libre exécute ses instructions jusqu'à une ACTION, qui prend du temps ; puis il recommence.",
        r.x + 20.0,
        r.y + 46.0,
        14.0,
        pal::DIM,
    );

    let col_w = (w - 60.0) / 2.0;
    let mut y = r.y + 76.0;
    let x = r.x + 20.0;
    let needed = 4.0 * 24.0 + vm::MNEMONICS.len() as f32 * 18.5;
    let k = ((h - 90.0) / needed).clamp(0.6, 1.0);
    let fs = 14.0 * k;
    let lh = 18.5 * k;
    for (cat, title) in [
        (Cat::Action, "ACTIONS  (prennent du temps, mettent ok à 1 ou 0)"),
        (Cat::Sense, "CAPTEURS  (instantanés)"),
        (Cat::Data, "DONNÉES"),
        (Cat::Flow, "SAUTS"),
    ] {
        gfx::text(title, x, y, 12.0 * k, token_color(vm::TokKind::Op(cat)));
        y += 18.0 * k;
        for m in vm::MNEMONICS.iter().filter(|m| m.cat == cat) {
            gfx::text(m.syntax, x + 8.0, y, fs, token_color(vm::TokKind::Op(cat)));
            gfx::text(m.help, x + 160.0 * k, y, fs, pal::TEXT);
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
        "Valeurs : registre, nombre (-3, 42) ou constante",
    ]);
    section(&mut y, "ANGLES (degrés, sens horaire)", &[
        "N=0  E=90  S=180  W=270  NONE=-1",
        "ADD a, 90 = tourner à droite ; MOD a, 360 pour rester dans 0..359",
        "x croît vers l'est, y vers le sud (POSX, POSY, DX, DY, ANGLE)",
    ]);
    section(&mut y, "CHOSES  (SCAN, DIST, DX, DY, COUNT, LOOK)", &[
        "EMPTY=0  WALL=1  ORE=2  ALLY=3  ENEMY=4  BASE=5  EBASE=6",
        &format!("portée des capteurs : {} unités ; EBASE : {} ; BASE : partout", SENSOR_RANGE / FP, EBASE_RANGE / FP),
        "DIST mesure l'écart entre les deux coques (0 = au contact)",
        &format!("FULL = {CARGO_CAP} (capacité de la soute)"),
    ]);
    section(&mut y, "PORTÉES", &[
        "MINE / DROP : DIST ≤ 1     ATTACK : DIST ≤ 2",
        "un MOVE = 1 unité ; WALL/ORE/usines bloquent, les drones se croisent",
    ]);
    section(&mut y, "MÉMOIRE DE RUCHE", &[
        &format!("{HIVE_MEM} cases partagées par les drones d'une usine (PEEK / POKE)"),
        "le bouton « Cible » d'une usine écrit 0 = X, 1 = Y, 2 = 1",
    ]);
    section(&mut y, "RACCOURCIS", &[
        "Espace pause  1-4 vitesse  N tick suivant  B usine  T cible",
        "E éditer/déboguer  P programmes  H base  Tab usines",
        "ZQSD/WASD/flèches, clic droit glissé : caméra ; molette : zoom",
        "Éditeur : Ctrl+S appliquer, Ctrl+Z/Y, Ctrl+D dupliquer, Tab",
    ]);
}
