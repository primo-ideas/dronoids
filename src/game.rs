//! One session in a world: drives the simulation (offline, or as a client
//! of the shared world), handles input and draws a deliberately quiet HUD,
//! the panels, the program editor and the debugger.

use std::collections::VecDeque;

use macroquad::prelude::*;

use dronoids::ai::{Bot, Level};
use dronoids::net::{ClientMsg, Conn, NetEvent, ServerMsg};
use dronoids::programs;
use dronoids::rules::*;
use dronoids::sim::quests::{self, QUESTS};
use dronoids::sim::{Command, Doing, Event, World};
use dronoids::vm::{self, Cat, Reg};

use crate::editor::{token_color, Editor};
use crate::gfx::{self, mix, pal, player_color, with_alpha, Bloom, Fog};
use crate::library::{self, Library};
use crate::platform::Settings;
use crate::ui::{TextField, Ui};
use crate::view::{draw_minimap, fp, fpv, Cam, Highlight, Sight, WorldView, FOG_UNKNOWN};

/// Hash checks happen this often (must match the server).
const HASH_EVERY: u64 = 50;
/// Past this many queued frames, ask for a fresh snapshot instead.
const MAX_BACKLOG: usize = 400;

pub enum Driver {
    Offline {
        bots: Vec<Bot>,
        pending: Vec<Command>,
    },
    Online {
        conn: Conn<ServerMsg>,
        frames: VecDeque<(u64, Vec<(u8, Command)>)>,
        lost: Option<String>,
        world_name: String,
        online: Vec<u8>,
        asked_snapshot: bool,
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
    ToggleMap,
    TogglePlayers,
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
    map_open: bool,
    players_open: bool,
    program_menu: Option<u32>,
    toasts: Vec<Toast>,
    follow: bool,
    over_dismissed: bool,
    step_once: bool,
    right_drag: Option<(Vec2, Vec2)>,
    /// What we can see (fog of war), refreshed every frame.
    sight: Sight,
    fog: Fog,
}

impl Game {
    fn new(world: World, me: u8, driver: Driver, lib: &Library) -> Game {
        let home = world.players.get(me as usize).map(|p| fpv(p.home.0, p.home.1)).unwrap_or(Vec2::splat(fp(world.size) / 2.0));
        let first = world.factories.iter().find(|f| f.owner == me);
        let center = first.map(|f| fpv(f.x, f.y)).unwrap_or(home);
        let start = first.map(|f| f.id);
        let sight = Sight::of(&world, me, 0.0);
        let mut g = Game {
            world,
            me,
            driver,
            cam: Cam { center, zoom: 24.0, shake: Vec2::ZERO },
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
            map_open: false,
            players_open: false,
            program_menu: None,
            toasts: vec![],
            follow: false,
            over_dismissed: false,
            step_once: false,
            right_drag: None,
            sight,
            fog: Fog::new(),
        };
        // A returning player's factory runs their own version of "mineur".
        if let (Some(fid), Some(src)) = (start, lib.source_or_builtin("mineur"))
            && g.world.factory(fid).is_some_and(|f| {
                g.world.programs[f.prog as usize].name == "mineur" && g.world.programs[f.prog as usize].source != src
            })
        {
            g.issue(Command::SetProgram { factory: fid, name: "mineur".into(), source: src });
        }
        g
    }

    /// A private world, for practice.
    pub fn offline(lib: &Library, settings: Settings, seed: u64, ais: usize, level: Level) -> Game {
        let mut world = World::new(seed);
        let me = world.join(&settings.name, false).unwrap_or(0);
        let bots = (0..ais)
            .filter_map(|i| world.join(&bot_name(i), true))
            .map(|id| Bot::new(id, level))
            .collect();
        Game::new(world, me, Driver::Offline { bots, pending: vec![] }, lib)
    }

    pub fn online(lib: &Library, conn: Conn<ServerMsg>, me: u8, world: World, world_name: String) -> Game {
        let driver = Driver::Online {
            conn,
            frames: VecDeque::new(),
            lost: None,
            world_name,
            online: vec![me],
            asked_snapshot: false,
        };
        Game::new(world, me, driver, lib)
    }

    fn is_online(&self) -> bool {
        matches!(self.driver, Driver::Online { .. })
    }

    fn issue(&mut self, cmd: Command) {
        match &mut self.driver {
            Driver::Offline { pending, .. } => pending.push(cmd),
            Driver::Online { conn, .. } => conn.send(&ClientMsg::Cmd(cmd)),
        }
    }

    fn toast(&mut self, text: impl Into<String>, color: Color) {
        self.toasts.push(Toast { text: text.into(), color, t: 0.0 });
        if self.toasts.len() > 3 {
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

    fn landing(&self) -> bool {
        self.world.players.get(self.me as usize).is_some_and(|p| p.alive) && self.world.is_landing(self.me)
    }

    // ----- simulation ------------------------------------------------------

    fn advance(&mut self, dt: f32) {
        let mut notes: Vec<Event> = vec![];
        let keep = |e: &Event| {
            matches!(
                e,
                Event::PlayerJoined { .. } | Event::PlayerEliminated { .. } | Event::FactoryDestroyed { .. } | Event::QuestDone { .. }
            )
        };
        match &mut self.driver {
            Driver::Offline { bots, pending } => {
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
                    self.view.on_events(&self.world, self.speed, &self.sight);
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
            Driver::Online { conn, frames, lost, online, asked_snapshot, .. } => {
                for ev in conn.poll() {
                    match ev {
                        NetEvent::Msg(ServerMsg::Frame { tick, cmds }) => frames.push_back((tick, cmds)),
                        NetEvent::Msg(ServerMsg::Resync { snapshot }) => match World::from_bytes(&snapshot) {
                            Ok(w) => {
                                self.world = w;
                                *asked_snapshot = false;
                            }
                            Err(e) => *lost = Some(format!("monde illisible : {e}")),
                        },
                        NetEvent::Msg(ServerMsg::Online { players }) => *online = players,
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
                // Far behind (a hidden tab, a slow machine): start again from a snapshot.
                if frames.len() > MAX_BACKLOG && !*asked_snapshot {
                    conn.send(&ClientMsg::WantSnapshot);
                    *asked_snapshot = true;
                }
                if *asked_snapshot {
                    return;
                }
                self.acc += dt * TICKS_PER_SEC;
                let max_steps = if frames.len() > 8 { 40 } else { 3 };
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
                    self.view.on_events(&self.world, 1.0, &self.sight);
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
                Event::PlayerJoined { player } if player != self.me && self.is_online() => {
                    let name = self.world.players[player as usize].name.clone();
                    self.toast(format!("{name} arrive dans le secteur"), player_color(player));
                }
                Event::PlayerEliminated { player } if player == self.me => {
                    self.toast("Votre colonie a disparu.", pal::BAD);
                }
                Event::FactoryDestroyed { owner, .. } if owner == self.me => {
                    self.toast("Une de vos usines a été détruite", pal::BAD);
                }
                Event::QuestDone { player, quest, reward } if player == self.me => {
                    let title = QUESTS.get(quest as usize).map_or("", |q| q.title);
                    let gain = if reward > 0 { format!(" · +{reward} cristaux") } else { String::new() };
                    self.toast(format!("Objectif atteint : {title}{gain}"), pal::ORE);
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
        let was_landing = self.landing();
        self.advance(dt);
        self.view.update(dt, &self.world);
        for t in &mut self.toasts {
            t.t += dt;
        }
        self.toasts.retain(|t| t.t < 4.0);
        self.validate_selection();
        if was_landing && !self.landing() {
            // First factory down: select it.
            if let Some(f) = self.world.factories.iter().find(|f| f.owner == self.me) {
                self.sel = Sel::Factory(f.id);
                self.tool = Tool::None;
            }
        }

        if self.follow
            && let Sel::Drone(id) = self.sel
            && let Some(p) = WorldView::drone_pos(&self.world, id, self.alpha())
        {
            self.cam.center = self.cam.center.lerp(p, (dt * 5.0).min(1.0));
        }
        self.cam.shake = self.view.shake_offset();

        self.sight = Sight::of(&self.world, self.me, self.alpha());
        bloom.begin();
        let hl = self.highlight(ui);
        self.view.draw(&self.world, &self.cam, self.alpha(), &hl, &self.sight);
        let (sight, cam) = (&self.sight, &self.cam);
        self.fog.apply(bloom, FOG_UNKNOWN, || sight.paint_mask(cam));
        bloom.finish();

        let mut acts = vec![];
        self.status(ui, &mut acts);
        self.objective();
        self.dock(ui, &mut acts);
        if self.players_open {
            self.players_panel(ui);
        }
        if self.editor_open {
            self.editor_window(ui, lib, &mut acts);
        } else {
            self.selection_panel(ui, lib, &mut acts);
            if self.map_open {
                let s = 190.0;
                let mm = Rect::new(screen_width() - s - 12.0, screen_height() - s - 12.0, s, s);
                ui.block(mm);
                if let Some(p) = draw_minimap(&self.world, &self.cam, mm, &self.sight, ui.mouse, ui.pressed || (ui.down && ui.hover(mm))) {
                    ui.pressed = false;
                    acts.push(Act::CenterOn(p));
                }
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
        // Someone else's unit that slips into the fog is no longer selected.
        let lost = match self.sel {
            Sel::Factory(id) => self.world.factory(id).is_some_and(|f| !self.sight.shows(f.owner, fpv(f.x, f.y))),
            Sel::Drone(id) => self.world.drone(id).is_some_and(|d| !self.sight.shows(d.owner, fpv(d.x, d.y))),
            Sel::None => false,
        };
        if lost {
            self.sel = Sel::None;
        }
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

    fn building(&self) -> bool {
        self.tool == Tool::Build || self.landing()
    }

    fn highlight(&self, ui: &Ui) -> Highlight {
        let ghost = if self.building() && !ui.over_ui() && !self.editor_open {
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
        let landing = self.landing().then(|| {
            let h = self.world.players[self.me as usize].home;
            fpv(h.0, h.1)
        });
        Highlight {
            factory: if let Sel::Factory(id) = self.sel { Some(id) } else { None },
            drone: if let Sel::Drone(id) = self.sel { Some(id) } else { None },
            ghost,
            build_range_for: (self.tool == Tool::Build && !self.landing()).then_some(self.me),
            target,
            landing,
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
            Act::ToggleMap => self.map_open = !self.map_open,
            Act::TogglePlayers => self.players_open = !self.players_open,
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
                    self.toast("Version d'origine restaurée (pas encore appliquée)", pal::WARN);
                }
            }
            Act::LoadProgram(name) => {
                if self.editor.dirty {
                    self.toast("Modifications non appliquées abandonnées", pal::WARN);
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
                    extra = format!(", usine #{fid}");
                }
                self.editor.dirty = false;
                self.toast(format!("« {name} » appliqué{extra}"), pal::GOOD);
            }
            Err(errs) => {
                self.toast(format!("Sauvegardé, mais {} erreur(s) : non appliqué", errs.len()), pal::BAD);
            }
        }
    }

    fn users_of(&self, name: &str) -> usize {
        self.world.factories.iter().filter(|f| f.owner == self.me && self.world.program_name(f.prog) == name).count()
    }

    fn editor_layout(&self) -> EditorLayout {
        let (sw, sh) = (screen_width(), screen_height());
        let x0 = (sw * 0.38).max(sw - 1000.0).min(sw - 560.0).max(10.0);
        let win = Rect::new(x0, 48.0, sw - x0 - 12.0, sh - 48.0 - 60.0);
        let header = Rect::new(win.x + 8.0, win.y + 8.0, win.w - 16.0, 30.0);
        let footer = Rect::new(win.x + 8.0, win.y + win.h - 62.0, win.w - 16.0, 54.0);
        let body_y = header.y + header.h + 8.0;
        let body_h = footer.y - 8.0 - body_y;
        let debug = matches!(self.edit_target, EditTarget::Debug(_));
        let list_w = if debug { 0.0 } else { 140.0 };
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
            let title = if alive { format!("Drone #{id}") } else { format!("Drone #{id} (détruit)") };
            gfx::text(&title, x, h.y + 7.0, 16.0, if alive { pal::ACCENT } else { pal::BAD });
            x += gfx::text_width(&title, 16.0) + 14.0;
            gfx::text(&format!("exécute « {} »", self.edit_name.text), x, h.y + 9.0, 13.0, pal::DIM);
            let mut bx = h.x + h.w - 30.0;
            if ui.button(Rect::new(bx, h.y, 30.0, bh), "×") {
                acts.push(Act::CloseEditor);
            }
            bx -= 128.0;
            if ui.button_ex(Rect::new(bx, h.y, 122.0, bh), "Éditer le code", false, true, pal::GOOD) {
                acts.push(Act::OpenProgram(self.edit_name.text.clone(), self.edit_factory));
            }
            if !self.is_online() {
                bx -= 128.0;
                if ui.button_ex(Rect::new(bx, h.y, 122.0, bh), "Pas à pas  N", false, true, pal::WARN) {
                    acts.push(Act::Step);
                }
            }
        } else {
            self.edit_name.ui(ui, Rect::new(x, h.y, 180.0, bh), "nom");
            if self.edit_name.focused {
                self.editor.focused = false;
            }
            x += 186.0;
            if self.editor.dirty {
                draw_circle(x + 6.0, h.y + bh / 2.0, 4.0, pal::WARN);
                if ui.hover(Rect::new(x, h.y, 14.0, bh)) {
                    ui.tooltip("Modifications non appliquées");
                }
            }
            x += 18.0;
            if ui.button_ex(Rect::new(x, h.y, 128.0, bh), "Appliquer · Ctrl+S", false, true, pal::GOOD) {
                acts.push(Act::Apply);
            }
            x += 134.0;
            let rest = [
                ("Nouveau", Act::NewProgram, true),
                ("Dupliquer", Act::Duplicate, true),
                ("Origine", Act::ResetBuiltin, Library::is_builtin(&self.edit_name.text)),
            ];
            for (label, act, enabled) in rest {
                let w = gfx::text_width(label, 13.0) + 20.0;
                if x + w > h.x + h.w - 70.0 {
                    break;
                }
                if ui.button_ex(Rect::new(x, h.y, w, bh), label, false, enabled, pal::ACCENT) {
                    acts.push(act);
                }
                x += w + 4.0;
            }
            if ui.button(Rect::new(h.x + h.w - 30.0, h.y, 30.0, bh), "×") {
                acts.push(Act::CloseEditor);
            }
            if ui.button(Rect::new(h.x + h.w - 64.0, h.y, 30.0, bh), "?") {
                acts.push(Act::ToggleHelp);
            }

            let lr = l.list;
            let mut y = lr.y + 4.0;
            for e in &lib.entries {
                if y + 24.0 > lr.y + lr.h {
                    break;
                }
                let r = Rect::new(lr.x, y, lr.w, 22.0);
                let current = e.name == self.edit_name.text;
                let used = self.users_of(&e.name);
                if current {
                    draw_rectangle(r.x, r.y, 2.0, r.h, pal::ACCENT);
                } else if ui.hover(r) {
                    draw_rectangle(r.x, r.y, r.w, r.h, with_alpha(pal::ACCENT, 0.06));
                }
                gfx::text(&e.name, r.x + 10.0, r.y + 4.0, 13.0, if current { pal::TEXT } else { pal::DIM });
                if used > 0 {
                    let s = format!("{used}");
                    gfx::text(&s, r.x + r.w - gfx::text_width(&s, 11.0) - 4.0, r.y + 5.0, 11.0, with_alpha(pal::GOOD, 0.8));
                }
                if !current && ui.click(r) {
                    acts.push(Act::LoadProgram(e.name.clone()));
                }
                y += 24.0;
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
        if debug_id.is_some() {
            gfx::text("La ligne surlignée est l'action en cours du drone.", f.x + 4.0, f.y + 8.0, 13.0, pal::DIM);
            if !self.is_online() {
                gfx::text("Espace : pause · N : un tick de plus", f.x + 4.0, f.y + 28.0, 13.0, pal::FAINT);
            }
        } else if self.editor.errors.is_empty() {
            let users = self.users_of(&self.edit_name.text);
            let msg = if users > 0 {
                format!("Appliquer met à jour {users} usine(s) « {} » et leurs drones.", self.edit_name.text)
            } else if let Some(fid) = self.edit_factory {
                format!("Appliquer donne ce programme à l'usine #{fid}.")
            } else {
                "Aucune usine n'utilise ce programme pour l'instant.".into()
            };
            gfx::text(&msg, f.x + 4.0, f.y + 8.0, 13.0, pal::DIM);
            gfx::text("Survolez une instruction pour son aide · F1 : tout le langage", f.x + 4.0, f.y + 28.0, 12.0, pal::FAINT);
        } else {
            for (i, e) in self.editor.errors.iter().take(2).enumerate() {
                let s = format!("ligne {} : {}", e.line + 1, e.msg);
                gfx::text(&s, f.x + 4.0, f.y + 8.0 + i as f32 * 19.0, 13.0, mix(pal::BAD, WHITE, 0.35));
            }
        }
    }

    fn debug_side(&self, ui: &mut Ui, r: Rect, id: u32, acts: &mut Vec<Act>) {
        let Some(d) = self.world.drone(id) else {
            gfx::text("Drone détruit.", r.x + 10.0, r.y + 10.0, 14.0, pal::BAD);
            return;
        };
        let mut y = r.y + 4.0;
        let x = r.x + 10.0;
        for reg in Reg::ALL.iter().chain([Reg::Ok].iter()) {
            let v = d.vm.reg(*reg);
            gfx::text(reg.name(), x, y, 16.0, token_color(vm::TokKind::Reg));
            gfx::text(&describe_value(v), x + 36.0, y, 16.0, pal::TEXT);
            y += 22.0;
        }
        y += 10.0;
        let (state, col) = doing_label(d.doing);
        gfx::text(&state, x, y, 13.0, col);
        y += 22.0;
        let rows = [
            ("soute", format!("{}/{}", d.cargo, CARGO_CAP)),
            ("cap", format!("{}°", d.heading)),
            ("position", format!("{}, {}", d.x / FP, d.y / FP)),
            ("PV", format!("{}/{}", d.hp, DRONE_HP)),
        ];
        for (k, v) in rows {
            gfx::text(k, x, y, 12.0, pal::DIM);
            gfx::text(&v, x + 74.0, y, 12.0, pal::TEXT);
            y += 18.0;
        }
        y += 10.0;
        gfx::text("mémoire de ruche", x, y, 11.0, pal::FAINT);
        y += 16.0;
        if let Some(f) = self.world.factory(d.factory) {
            let cw = (r.w - 20.0) / 4.0;
            for i in 0..HIVE_MEM {
                let cx = x + (i % 4) as f32 * cw;
                let cy = y + (i / 4) as f32 * 22.0;
                let v = f.mem[i];
                gfx::text(&format!("{v}"), cx + 2.0, cy + 2.0, 12.0, if v != 0 { pal::TEXT } else { pal::FAINT });
            }
            y += 4.0 * 22.0 + 6.0;
        }
        let fr = Rect::new(x, y.min(r.y + r.h - 32.0), r.w - 20.0, 26.0);
        if ui.button_ex(fr, if self.follow { "La caméra le suit" } else { "Suivre ce drone" }, self.follow, true, pal::ACCENT) {
            acts.push(Act::Follow(!self.follow));
        }
    }

    // ----- HUD --------------------------------------------------------------

    /// Top-left: who we are and what we have. Top-right: a few small buttons.
    fn status(&self, ui: &mut Ui, acts: &mut Vec<Act>) {
        let Some(me) = self.world.players.get(self.me as usize) else { return };
        let pc = player_color(self.me);
        let (x, y) = (16.0, 14.0);
        draw_poly(x + 6.0, y + 8.0, 6, 6.0, 30.0, with_alpha(pc, 0.9));
        gfx::text(&me.name, x + 18.0, y, 15.0, mix(pc, WHITE, 0.25));
        let mins = me.minerals.to_string();
        gfx::text(&mins, x + 18.0, y + 20.0, 13.0, pal::ORE);
        let nf = self.world.factory_count(self.me);
        let line = format!(
            " cristaux  ·  {} drones  ·  {nf} usine{}",
            self.world.drone_count(self.me),
            if nf > 1 { "s" } else { "" }
        );
        gfx::text(&line, x + 18.0 + gfx::text_width(&mins, 13.0), y + 20.0, 13.0, pal::DIM);
        if self.world.shielded(self.me) {
            let left = (me.shield_until - self.world.tick) as f32 / TICKS_PER_SEC;
            let s = format!("protégé encore {}:{:02}", left as u32 / 60, left as u32 % 60);
            gfx::text(&s, x + 18.0, y + 38.0, 12.0, with_alpha(pal::ACCENT, 0.7));
        }

        let sw = screen_width();
        let mut rx = sw - 12.0;
        let small = |ui: &mut Ui, rx: &mut f32, label: &str, active: bool, color: Color| -> bool {
            let w = gfx::text_width(label, 13.0) + 18.0;
            *rx -= w;
            let hit = ui.button_ex(Rect::new(*rx, 10.0, w, 26.0), label, active, true, color);
            *rx -= 4.0;
            hit
        };
        if small(ui, &mut rx, "Menu", false, pal::ACCENT) {
            acts.push(Act::Menu);
        }
        if small(ui, &mut rx, "?", self.help_open, pal::ACCENT) {
            acts.push(Act::ToggleHelp);
        }
        match &self.driver {
            Driver::Offline { .. } => {
                for (s, label) in [(8.0, "x8"), (4.0, "x4"), (2.0, "x2"), (1.0, "x1"), (0.0, "II")] {
                    let active = (self.speed - s).abs() < 0.01;
                    if small(ui, &mut rx, label, active, pal::ACCENT) {
                        acts.push(Act::Speed(s));
                    }
                }
            }
            Driver::Online { frames, lost, online, .. } => {
                let (s, c) = if lost.is_some() {
                    ("hors ligne".to_string(), pal::BAD)
                } else if frames.len() > 8 {
                    ("synchronisation…".to_string(), pal::WARN)
                } else {
                    (format!("{} en ligne", online.len()), pal::GOOD)
                };
                let w = gfx::text_width(&s, 12.0);
                rx -= w + 20.0;
                draw_circle(rx + 4.0, 23.0, 3.0, c);
                gfx::text(&s, rx + 12.0, 16.0, 12.0, pal::DIM);
            }
        }
    }

    /// One discreet line of guidance, only when it is useful: what the
    /// current tool expects, or else the next first-steps objective.
    fn objective(&self) {
        let pulse = with_alpha(pal::TEXT, 0.75 + 0.25 * (get_time() as f32 * 2.0).sin());
        let parts: Vec<(String, Color)> = match self.tool {
            Tool::Build => vec![("Nouvelle usine : près de vos unités · clic droit pour annuler".into(), pulse)],
            Tool::Target(_) => vec![("Cible : cliquez sur la carte · clic droit pour annuler".into(), pulse)],
            Tool::None if self.landing() => {
                vec![("Posez votre première usine : cliquez sur un espace libre (le cercle est un bon endroit)".into(), pulse)]
            }
            Tool::None => {
                let Some(i) = quests::current(&self.world, self.me) else { return };
                let q = &QUESTS[i];
                let (done, goal) = quests::progress(&self.world, self.me, i);
                let mut parts = vec![("Objectif  ".to_string(), pal::DIM), (q.title.to_string(), pal::TEXT)];
                if goal > 100 {
                    parts.push((format!("  {} %", done * 100 / goal), pal::ACCENT));
                } else if goal > 1 {
                    parts.push((format!("  {done}/{goal}"), pal::ACCENT));
                }
                parts.push((format!("  ·  {}", q.hint), pal::DIM));
                if q.reward > 0 {
                    parts.push((format!("  +{}", q.reward), pal::ORE));
                }
                parts
            }
        };
        let size = 14.0;
        let w: f32 = parts.iter().map(|(t, _)| gfx::text_width(t, size)).sum();
        let mut x = screen_width() / 2.0 - w / 2.0;
        draw_rectangle(x - 14.0, 56.0, w + 28.0, 26.0, with_alpha(pal::PANEL, 0.6));
        for (t, c) in &parts {
            gfx::text(t, x, 61.0, size, *c);
            x += gfx::text_width(t, size);
        }
    }

    /// Bottom-centre: a handful of quiet buttons.
    fn dock(&self, ui: &mut Ui, acts: &mut Vec<Act>) {
        let Some(me) = self.world.players.get(self.me as usize) else { return };
        let can = me.minerals >= FACTORY_COST && me.alive && !self.landing();
        let items: [(String, bool, bool, Color, Act); 5] = [
            (format!("Usine · {FACTORY_COST}"), self.tool == Tool::Build, can || self.tool == Tool::Build, pal::GOOD, Act::Tool(Tool::Build)),
            ("Programmes".into(), self.editor_open, true, pal::ACCENT, Act::OpenProgram(self.default_program_name(), None)),
            ("Carte".into(), self.map_open, true, pal::ACCENT, Act::ToggleMap),
            ("Colonies".into(), self.players_open, true, pal::ACCENT, Act::TogglePlayers),
            ("Base".into(), false, !self.landing(), pal::ACCENT, Act::CenterOn(self.home())),
        ];
        let widths: Vec<f32> = items.iter().map(|i| gfx::text_width(&i.0, 13.0) + 26.0).collect();
        let total: f32 = widths.iter().sum::<f32>() + 6.0 * (items.len() as f32 - 1.0);
        let mut x = screen_width() / 2.0 - total / 2.0;
        let y = screen_height() - 42.0;
        for ((label, active, enabled, color, act), w) in items.into_iter().zip(widths) {
            if ui.button_ex(Rect::new(x, y, w, 30.0), &label, active, enabled, color) {
                acts.push(match act {
                    Act::OpenProgram(..) if self.editor_open => Act::CloseEditor,
                    a => a,
                });
            }
            x += w + 6.0;
        }
    }

    fn players_panel(&self, ui: &mut Ui) {
        let mut rows: Vec<(u8, usize, usize, bool)> = self
            .world
            .players
            .iter()
            .map(|p| (p.id, self.world.factory_count(p.id), self.world.drone_count(p.id), p.alive))
            .collect();
        rows.sort_by_key(|r| (!r.3, std::cmp::Reverse(r.1 * 20 + r.2), r.0));
        let shown = rows.len().min(14);
        let r = Rect::new(16.0, 70.0, 260.0, 30.0 + shown as f32 * 19.0);
        ui.panel(r);
        let title = match &self.driver {
            Driver::Online { world_name, .. } => format!("Secteur « {world_name} »"),
            Driver::Offline { .. } => "Entraînement".into(),
        };
        gfx::text(&title, r.x + 10.0, r.y + 7.0, 12.0, pal::DIM);
        let online: &[u8] = match &self.driver {
            Driver::Online { online, .. } => online,
            _ => &[],
        };
        for (i, (id, f, d, alive)) in rows.iter().take(shown).enumerate() {
            let y = r.y + 26.0 + i as f32 * 19.0;
            let p = &self.world.players[*id as usize];
            let c = if *alive { player_color(*id) } else { pal::FAINT };
            draw_poly(r.x + 15.0, y + 7.0, 6, 4.0, 30.0, c);
            let tc = if *id == self.me { pal::TEXT } else if *alive { mix(c, pal::TEXT, 0.6) } else { pal::FAINT };
            gfx::text(&p.name, r.x + 26.0, y, 13.0, tc);
            if online.contains(id) && *id != self.me {
                draw_circle(r.x + 30.0 + gfx::text_width(&p.name, 13.0) + 4.0, y + 7.0, 2.5, pal::GOOD);
            }
            let info = if *alive { format!("{f} · {d}") } else { "—".into() };
            gfx::text(&info, r.x + r.w - 10.0 - gfx::text_width(&info, 12.0), y + 1.0, 12.0, pal::DIM);
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
        let w = 290.0;
        let x = screen_width() - w - 12.0;
        let y = 48.0;
        match self.sel {
            Sel::None => {}
            Sel::Factory(id) => {
                let Some(f) = self.world.factory(id) else { return };
                let mine = f.owner == self.me;
                let r = Rect::new(x, y, w, if mine { 196.0 } else { 74.0 });
                ui.panel(r);
                let pc = player_color(f.owner);
                let owner = &self.world.players[f.owner as usize].name;
                gfx::text(&format!("Usine #{id}"), x + 12.0, y + 10.0, 15.0, mix(pc, WHITE, 0.2));
                gfx::text(owner, x + w - 12.0 - gfx::text_width(owner, 12.0), y + 13.0, 12.0, pal::DIM);
                thin_bar(x + 12.0, y + 34.0, w - 24.0, f.hp as f32 / FACTORY_HP as f32, mix(pal::BAD, pal::GOOD, f.hp as f32 / FACTORY_HP as f32));
                let pname = self.world.program_name(f.prog);
                gfx::text("programme", x + 12.0, y + 46.0, 12.0, pal::DIM);
                gfx::text(pname, x + 90.0, y + 45.0, 14.0, pal::ACCENT);
                if !mine {
                    return;
                }
                let mut cy = y + 70.0;
                if ui.button_ex(Rect::new(x + 12.0, cy, 128.0, 28.0), "Programme · E", false, true, pal::GOOD) {
                    acts.push(Act::OpenProgram(pname.to_string(), Some(id)));
                }
                if ui.button_ex(Rect::new(x + 146.0, cy, 132.0, 28.0), "Changer…", self.program_menu == Some(id), true, pal::ACCENT) {
                    acts.push(Act::ToggleProgramMenu(id));
                }
                cy += 38.0;
                let n = self.world.drones_of_factory(id);
                gfx::text(&format!("drones {n}/{}", f.quota), x + 12.0, cy + 7.0, 13.0, pal::TEXT);
                if ui.button(Rect::new(x + 112.0, cy, 28.0, 26.0), "−") && f.quota > 0 {
                    acts.push(Act::Issue(Command::SetQuota { factory: id, quota: f.quota - 1 }));
                }
                if ui.button(Rect::new(x + 144.0, cy, 28.0, 26.0), "+") && f.quota < MAX_QUOTA {
                    acts.push(Act::Issue(Command::SetQuota { factory: id, quota: f.quota + 1 }));
                }
                let (label, col) = if f.enabled { ("en production", pal::GOOD) } else { ("en pause", pal::WARN) };
                if ui.button_ex(Rect::new(x + 180.0, cy, 98.0, 26.0), label, false, true, col) {
                    acts.push(Act::Issue(Command::ToggleFactory { factory: id }));
                }
                cy += 36.0;
                let targeting = self.tool == Tool::Target(id);
                let tl = if f.mem[2] == 1 { format!("cible {}, {}", f.mem[0], f.mem[1]) } else { "pas de cible".into() };
                gfx::text(&tl, x + 12.0, cy + 7.0, 13.0, if f.mem[2] == 1 { pal::TEXT } else { pal::DIM });
                if ui.button_ex(Rect::new(x + 146.0, cy, 98.0, 26.0), "Cibler · T", targeting, true, pal::PINK) {
                    acts.push(Act::Tool(Tool::Target(id)));
                }
                if ui.hover(Rect::new(x + 146.0, cy, 98.0, 26.0)) {
                    ui.tooltip("Écrit X, Y dans la mémoire 0 et 1 de l'usine (2 = 1) : « chasseur » s'y rend");
                }
                if ui.button_ex(Rect::new(x + 250.0, cy, 28.0, 26.0), "×", false, f.mem[2] != 0, pal::PINK) {
                    acts.push(Act::Issue(Command::SetMemory { factory: id, index: 2, value: 0 }));
                }

                if self.program_menu == Some(id) {
                    let n = lib.entries.len().min(14);
                    let pr = Rect::new(x - 172.0, y + 70.0, 164.0, 10.0 + n as f32 * 26.0);
                    ui.panel(pr);
                    for (i, e) in lib.entries.iter().take(n).enumerate() {
                        let rr = Rect::new(pr.x + 5.0, pr.y + 5.0 + i as f32 * 26.0, pr.w - 10.0, 24.0);
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
                let r = Rect::new(x, y, w, if mine { 132.0 } else { 74.0 });
                ui.panel(r);
                let pc = player_color(d.owner);
                gfx::text(&format!("Drone #{id}"), x + 12.0, y + 10.0, 15.0, mix(pc, WHITE, 0.2));
                let owner = &self.world.players[d.owner as usize].name;
                gfx::text(owner, x + w - 12.0 - gfx::text_width(owner, 12.0), y + 13.0, 12.0, pal::DIM);
                thin_bar(x + 12.0, y + 34.0, w - 24.0, d.hp as f32 / DRONE_HP as f32, mix(pal::BAD, pal::GOOD, d.hp as f32 / DRONE_HP as f32));
                let (state, col) = doing_label(d.doing);
                gfx::text(&format!("{state} · soute {}/{CARGO_CAP}", d.cargo), x + 12.0, y + 46.0, 13.0, col);
                if !mine {
                    return;
                }
                gfx::text(&format!("programme « {} »", self.world.program_name(d.prog)), x + 12.0, y + 66.0, 12.0, pal::DIM);
                if ui.button_ex(Rect::new(x + 12.0, y + 92.0, 128.0, 28.0), "Déboguer · E", false, true, pal::GOOD) {
                    acts.push(Act::OpenDebug(id));
                }
                if ui.button_ex(Rect::new(x + 146.0, y + 92.0, 132.0, 28.0), "Suivre", self.follow, true, pal::ACCENT) {
                    acts.push(Act::Follow(!self.follow));
                }
            }
        }
    }

    fn draw_toasts(&self) {
        let mut y = screen_height() - 58.0;
        for t in self.toasts.iter().rev() {
            let a = (1.0 - ((t.t - 3.2) / 0.8).max(0.0)).clamp(0.0, 1.0) * (t.t * 5.0).min(1.0);
            let size = 13.0;
            let w = gfx::text_width(&t.text, size) + 28.0;
            let r = Rect::new(screen_width() / 2.0 - w / 2.0, y - 28.0, w, 24.0);
            draw_rectangle(r.x, r.y, r.w, r.h, with_alpha(pal::PANEL, 0.75 * a));
            draw_circle(r.x + 10.0, r.y + 12.0, 2.5, with_alpha(t.color, a));
            gfx::text(&t.text, r.x + 18.0, r.y + 5.0, size, with_alpha(mix(t.color, pal::TEXT, 0.6), a));
            y -= 28.0;
        }
    }

    fn overlays(&self, ui: &mut Ui, acts: &mut Vec<Act>) {
        let (sw, sh) = (screen_width(), screen_height());
        if let Driver::Online { lost: Some(msg), .. } = &self.driver {
            let r = Rect::new(sw / 2.0 - 230.0, sh / 2.0 - 70.0, 460.0, 140.0);
            ui.panel(r);
            gfx::text_centered("Connexion interrompue", sw / 2.0, r.y + 20.0, 18.0, pal::WARN);
            gfx::text_centered(msg, sw / 2.0, r.y + 54.0, 13.0, pal::DIM);
            if ui.button(Rect::new(sw / 2.0 - 70.0, r.y + 92.0, 140.0, 30.0), "Retour au menu") {
                acts.push(Act::Menu);
            }
            return;
        }
        let me_alive = self.world.players.get(self.me as usize).is_some_and(|p| p.alive);
        if !me_alive {
            let r = Rect::new(sw / 2.0 - 220.0, sh / 2.0 - 80.0, 440.0, 160.0);
            ui.panel(r);
            gfx::text_centered("Votre colonie a disparu", sw / 2.0, r.y + 22.0, 20.0, pal::BAD);
            gfx::text_centered("Le secteur continue sans vous… pour l'instant.", sw / 2.0, r.y + 60.0, 13.0, pal::DIM);
            if ui.button_ex(Rect::new(sw / 2.0 - 150.0, r.y + 104.0, 144.0, 32.0), "Recommencer", false, true, pal::GOOD) {
                acts.push(Act::Respawn);
            }
            if ui.button(Rect::new(sw / 2.0 + 6.0, r.y + 104.0, 144.0, 32.0), "Menu") {
                acts.push(Act::Menu);
            }
        }
        // Offline practice against colonies can be won.
        if !self.is_online() && me_alive && !self.over_dismissed && self.world.winner() == Some(self.me) {
            let r = Rect::new(sw / 2.0 - 200.0, sh / 2.0 - 70.0, 400.0, 140.0);
            ui.panel(r);
            gfx::text_centered("Secteur conquis", sw / 2.0, r.y + 22.0, 22.0, pal::GOOD);
            if ui.button(Rect::new(sw / 2.0 - 150.0, r.y + 90.0, 144.0, 30.0), "Continuer") {
                acts.push(Act::DismissOver);
            }
            if ui.button(Rect::new(sw / 2.0 + 6.0, r.y + 90.0, 144.0, 30.0), "Menu") {
                acts.push(Act::Menu);
            }
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
        if self.building() {
            match self.world.can_place_factory(self.me, mx, my) {
                Ok(()) => {
                    let name = self.default_program_name();
                    let source = lib.source_or_builtin(&name).unwrap_or_else(|| programs::TEMPLATE.into());
                    acts.push(Act::Issue(Command::PlaceFactory { x: mx, y: my, name, source }));
                    if !ui.shift {
                        self.tool = Tool::None;
                    }
                }
                Err(why) => self.toast(format!("Impossible ici : {why}"), pal::WARN),
            }
            return;
        }
        if let Tool::Target(fid) = self.tool {
            for (i, v) in [(0u8, mx / FP), (1, my / FP), (2, 1)] {
                acts.push(Act::Issue(Command::SetMemory { factory: fid, index: i, value: v }));
            }
            self.tool = Tool::None;
            return;
        }
        let wp = self.cam.s2w(ui.mouse);
        let alpha = self.alpha();
        let pick = (0.9f32).max(10.0 / self.cam.zoom);
        let drone = self
            .world
            .drones
            .iter()
            .filter_map(|d| WorldView::drone_pos(&self.world, d.id, alpha).map(|p| (d.id, p.distance(wp))))
            .filter(|(id, dist)| *dist < pick && self.world.drone(*id).is_some_and(|d| self.sight.shows(d.owner, fpv(d.x, d.y))))
            .min_by(|a, b| a.1.total_cmp(&b.1));
        let factory = self
            .world
            .factories
            .iter()
            .map(|f| (f.id, fpv(f.x, f.y).distance(wp)))
            .filter(|(id, d)| *d < fp(FACTORY_RADIUS) + 0.4 && self.world.factory(*id).is_some_and(|f| self.sight.shows(f.owner, fpv(f.x, f.y))))
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
        if is_key_pressed(KeyCode::B) && !self.landing() {
            acts.push(Act::Tool(Tool::Build));
        }
        if is_key_pressed(KeyCode::M) {
            acts.push(Act::ToggleMap);
        }
        if is_key_pressed(KeyCode::L) {
            acts.push(Act::TogglePlayers);
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
        if let Driver::Offline { bots, pending } = &mut self.driver {
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

    /// Land automatically (screenshots of a running colony).
    pub fn auto_land(&mut self) {
        let (hx, hy) = self.world.players[self.me as usize].home;
        let m = programs::builtin("mineur").unwrap();
        if let Some(p) = (0..10)
            .flat_map(|r| (0..360).step_by(30).map(move |a| (r, a)))
            .map(|(r, a)| {
                let (dx, dy) = dronoids::geo::dir_vec(a, r * 3 * FP);
                (hx + dx, hy + dy)
            })
            .find(|p| self.world.can_place_factory(self.me, p.0, p.1).is_ok())
        {
            self.issue(Command::PlaceFactory { x: p.0, y: p.1, name: m.name.into(), source: m.source.into() });
            self.fast_forward(1);
            self.cam.center = fpv(p.0, p.1);
            self.sel = self.world.factories.iter().find(|f| f.owner == self.me).map(|f| Sel::Factory(f.id)).unwrap_or(Sel::None);
        }
    }

    pub fn open_program_by_name(&mut self, lib: &Library, name: &str) {
        let fid = self.world.factories.iter().find(|f| f.owner == self.me).map(|f| f.id);
        self.sel = fid.map(Sel::Factory).unwrap_or(Sel::None);
        self.open_program(lib, name, fid);
    }

    pub fn set_overlays(&mut self, help: bool, build: bool, map: bool, players: bool) {
        self.help_open = help;
        self.map_open = map;
        self.players_open = players;
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

pub fn bot_name(i: usize) -> String {
    const NAMES: &[&str] = &["Orion", "Vega", "Lyra", "Rigel", "Deneb", "Sirius", "Altaïr", "Antarès"];
    format!("IA {}", NAMES[i % NAMES.len()])
}

fn doing_label(d: Doing) -> (String, Color) {
    match d {
        Doing::Idle => ("réfléchit".into(), pal::TEXT),
        Doing::Move { .. } => ("avance".into(), pal::TEXT),
        Doing::Mine { .. } => ("mine".into(), pal::ORE),
        Doing::Drop { .. } => ("décharge".into(), pal::ORE),
        Doing::Attack { .. } => ("tire".into(), pal::BAD),
        Doing::Wait => ("attend".into(), pal::DIM),
        Doing::Fail => ("bloqué".into(), pal::WARN),
        Doing::Stall => ("calcule…".into(), pal::WARN),
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
    if name.is_empty() { format!("{v}") } else { format!("{v}  {name}") }
}

fn thin_bar(x: f32, y: f32, w: f32, frac: f32, color: Color) {
    draw_rectangle(x, y, w, 3.0, with_alpha(pal::FAINT, 0.6));
    draw_rectangle(x, y, w * frac.clamp(0.0, 1.0), 3.0, with_alpha(color, 0.85));
}

/// Full language reference.
fn help_overlay(ui: &mut Ui, acts: &mut Vec<Act>) {
    let (sw, sh) = (screen_width(), screen_height());
    let w = (sw - 80.0).min(1240.0);
    let h = (sh - 80.0).min(820.0);
    let r = Rect::new((sw - w) / 2.0, (sh - h) / 2.0, w, h);
    ui.panel(r);
    if ui.button(Rect::new(r.x + r.w - 40.0, r.y + 10.0, 30.0, 28.0), "×") {
        acts.push(Act::ToggleHelp);
    }
    gfx::text("DASM, le langage des drones", r.x + 20.0, r.y + 16.0, 18.0, pal::ACCENT);
    gfx::text(
        "Un drone libre exécute ses instructions jusqu'à une ACTION, qui prend du temps ; puis il recommence.",
        r.x + 20.0,
        r.y + 42.0,
        13.0,
        pal::DIM,
    );

    let col_w = (w - 60.0) / 2.0;
    let mut y = r.y + 72.0;
    let x = r.x + 20.0;
    let needed = 4.0 * 24.0 + vm::MNEMONICS.len() as f32 * 18.5;
    let k = ((h - 90.0) / needed).clamp(0.6, 1.0);
    let fs = 13.5 * k;
    let lh = 18.5 * k;
    for (cat, title) in [
        (Cat::Action, "actions · prennent du temps, mettent ok à 1 ou 0"),
        (Cat::Sense, "capteurs · instantanés"),
        (Cat::Data, "données"),
        (Cat::Flow, "sauts"),
    ] {
        gfx::text(title, x, y, 12.0 * k, with_alpha(token_color(vm::TokKind::Op(cat)), 0.8));
        y += 18.0 * k;
        for m in vm::MNEMONICS.iter().filter(|m| m.cat == cat) {
            gfx::text(m.syntax, x + 8.0, y, fs, token_color(vm::TokKind::Op(cat)));
            gfx::text(m.help, x + 160.0 * k, y, fs, pal::TEXT);
            y += lh;
        }
        y += 6.0 * k;
    }

    let x2 = r.x + 40.0 + col_w;
    let mut y = r.y + 72.0;
    let section = |y: &mut f32, title: &str, lines: &[&str]| {
        gfx::text(title, x2, *y, 12.0 * k, with_alpha(pal::VIOLET, 0.85));
        *y += 18.0 * k;
        for l in lines {
            gfx::text(l, x2 + 8.0, *y, fs, pal::TEXT);
            *y += lh;
        }
        *y += 6.0 * k;
    };
    section(&mut y, "registres", &[
        "a b c d   registres libres du drone",
        "ok        1 si la dernière action a réussi, sinon 0",
        "valeurs : registre, nombre (-3, 42) ou constante",
    ]);
    section(&mut y, "angles · degrés, sens horaire", &[
        "N=0  E=90  S=180  W=270  NONE=-1",
        "ADD a, 90 = tourner à droite ; MOD a, 360 pour rester dans 0..359",
        "x croît vers l'est, y vers le sud (POSX, POSY, DX, DY, ANGLE)",
    ]);
    section(&mut y, "choses · SCAN, DIST, DX, DY, COUNT, LOOK", &[
        "EMPTY=0  WALL=1  ORE=2  ALLY=3  ENEMY=4  BASE=5  EBASE=6",
        &format!("capteurs : {} unités ; EBASE : {} ; BASE : partout", SENSOR_RANGE / FP, EBASE_RANGE / FP),
        "DIST mesure l'écart entre les deux coques (0 = au contact)",
        &format!("FULL = {CARGO_CAP} (capacité de la soute)"),
    ]);
    section(&mut y, "portées", &[
        "MINE / DROP : DIST ≤ 1     ATTACK : DIST ≤ 2",
        "un MOVE = 1 unité ; astéroïdes, cristaux et usines bloquent",
    ]);
    section(&mut y, "mémoire de ruche", &[
        &format!("{HIVE_MEM} cases partagées par les drones d'une usine (PEEK / POKE)"),
        "« Cibler » écrit 0 = X, 1 = Y, 2 = 1",
    ]);
    section(&mut y, "raccourcis", &[
        "B usine · T cible · E programme/débogueur · P programmes",
        "M carte · L colonies · H base · Tab usines suivantes",
        "ZQSD/WASD/flèches, clic droit glissé : caméra ; molette : zoom",
        "éditeur : Ctrl+S appliquer, Ctrl+Z/Y, Ctrl+D, Tab",
    ]);
}
