//! One session in a world: drives the simulation (offline, or as a client
//! of the shared world), handles input and draws a deliberately quiet HUD,
//! the panels, the program editor and the debugger.

use std::collections::{HashMap, VecDeque};

use macroquad::prelude::*;

use dronoids::ai::{Bot, Level};
use dronoids::net::{ClientMsg, Conn, NetEvent, ServerMsg};
use dronoids::programs;
use dronoids::rules::*;
use dronoids::sim::quests::{self, QUESTS};
use dronoids::sim::{Command, Doing, Event, World};
use dronoids::vm::{self, Cat, Reg};

use crate::blocks::Blocks;
use crate::editor::{token_color, Editor};
use crate::gfx::{self, mix, pal, player_color, with_alpha, Bloom, Fog};
use crate::library::{self, Library};
use crate::platform::Settings;
use crate::ui::{self, TextField, Ui};
use crate::view::{draw_minimap, fp, fpv, Cam, Highlight, Sight, WorldView, FOG_UNKNOWN, MAX_ZOOM, MIN_ZOOM};

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
    Base(u32),
    Dronoid(u32),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tool {
    None,
    /// Pick the point the base's hunters head for.
    Target,
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
    /// A program, and the production line it is for, if any.
    OpenProgram(String, Option<u32>),
    OpenDebug(u32),
    Select(Sel),
    Speed(f32),
    Step,
    Tool(Tool),
    ToggleHelp,
    ToggleMap,
    TogglePlayers,
    /// The list of programs to give a production line.
    ToggleProgramMenu(u32),
    CloseEditor,
    Apply,
    NewProgram,
    Duplicate,
    ResetBuiltin,
    /// Between the tiles and the text of the program.
    ToggleTextMode,
    LoadProgram(String),
    CenterOn(Vec2),
    Follow(bool),
    DismissOver,
    Respawn,
    Menu,
}

/// What was asked of a production line and is not yet in the world: a
/// command takes a server tick and a frame to come back, and the panel shows
/// the new setting meanwhile (and counts the next click from it).
#[derive(Default)]
struct Pending {
    quota: Option<u8>,
    enabled: Option<bool>,
    program: Option<String>,
    /// When it was last asked (`get_time`).
    at: f64,
}

/// Shown as asked at least this long, and given up after that long if the
/// world never agrees (a command the server refused).
const PENDING_MIN: f64 = 1.5;
const PENDING_MAX: f64 = 6.0;

struct Toast {
    text: String,
    color: Color,
    t: f32,
}

struct EditorLayout {
    win: Rect,
    header: Rect,
    /// A second row of buttons, on a small screen.
    header2: Option<Rect>,
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
    /// The production line whose program is in the editor.
    edit_line: Option<u32>,
    debug_prog: u32,
    help_open: bool,
    map_open: bool,
    players_open: bool,
    /// The production line whose list of programs is open.
    program_menu: Option<u32>,
    toasts: Vec<Toast>,
    /// My base's hull, to notice a raid, and when the last warning was given.
    hull: Option<i32>,
    warned_at: f64,
    /// Line settings asked for, by line id, until the world shows them.
    pending: HashMap<u32, Pending>,
    follow: bool,
    over_dismissed: bool,
    step_once: bool,
    right_drag: Option<(Vec2, Vec2)>,
    /// The fingers on the screen landed on the map (not on a panel).
    touch_world: bool,
    /// The program list, folded away in the editor on small screens.
    list_open: bool,
    help_scroll: f32,
    /// The program as tiles (the default), or as text.
    blocks: Blocks,
    text_mode: bool,
    /// What we can see (fog of war), refreshed every frame.
    sight: Sight,
    fog: Fog,
}

impl Game {
    fn new(world: World, me: u8, driver: Driver, lib: &Library) -> Game {
        let home = world.players.get(me as usize).map(|p| fpv(p.home.0, p.home.1)).unwrap_or(Vec2::splat(fp(world.size) / 2.0));
        let base = world.base_of(me);
        let center = base.map(|b| fpv(b.x, b.y)).unwrap_or(home);
        let start = base.map(|b| b.id);
        let sight = Sight::of(&world, me, 0.0, Rect::new(center.x - 300.0, center.y - 300.0, 600.0, 600.0));
        let mut g = Game {
            world,
            me,
            driver,
            cam: Cam { center, zoom: 20.0, shake: Vec2::ZERO },
            speed: 1.0,
            acc: 0.0,
            view: WorldView::default(),
            sel: start.map(Sel::Base).unwrap_or(Sel::None),
            tool: Tool::None,
            editor_open: false,
            editor: Editor::new(),
            edit_name: TextField::new("", 24, library::name_char_ok),
            edit_target: EditTarget::Program,
            edit_line: None,
            debug_prog: u32::MAX,
            help_open: false,
            map_open: false,
            players_open: false,
            program_menu: None,
            toasts: vec![],
            hull: None,
            warned_at: -60.0,
            pending: HashMap::new(),
            follow: false,
            over_dismissed: false,
            step_once: false,
            right_drag: None,
            touch_world: false,
            list_open: false,
            help_scroll: 0.0,
            blocks: Blocks::default(),
            text_mode: false,
            sight,
            fog: Fog::new(),
        };
        // A returning player's lines run their own version of "miner".
        if let Some(src) = lib.source_or_builtin("miner") {
            let stale: Vec<u32> = g
                .world
                .base_of(me)
                .map(|b| {
                    b.lines
                        .iter()
                        .filter(|l| g.world.programs[l.prog as usize].name == "miner" && g.world.programs[l.prog as usize].source != src)
                        .map(|l| l.id)
                        .collect()
                })
                .unwrap_or_default();
            for line in stale {
                g.issue(Command::SetProgram { line, name: "miner".into(), source: src.clone() });
            }
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
        self.expect(&cmd);
        match &mut self.driver {
            Driver::Offline { pending, .. } => pending.push(cmd),
            Driver::Online { conn, .. } => conn.send(&ClientMsg::Cmd(cmd)),
        }
    }

    /// Remember what a command changes on one of my lines, to show it now.
    fn expect(&mut self, cmd: &Command) {
        let line = match cmd {
            Command::SetQuota { line, .. } | Command::ToggleLine { line } | Command::SetProgram { line, .. } => *line,
            _ => return,
        };
        let Some(l) = self.world.base_of(self.me).and_then(|b| b.line(line)) else { return };
        let enabled = self.shown_line(l).0.enabled;
        let p = self.pending.entry(line).or_default();
        p.at = get_time();
        match cmd {
            Command::SetQuota { quota, .. } => p.quota = Some((*quota).min(MAX_QUOTA)),
            Command::ToggleLine { .. } => p.enabled = Some(!enabled),
            Command::SetProgram { name, .. } => p.program = Some(name.clone()),
            _ => {}
        }
    }

    /// A line as the panel shows it: what the world says, with what was
    /// asked and is not there yet; and its program's name.
    fn shown_line(&self, l: &dronoids::sim::Line) -> (dronoids::sim::Line, String) {
        let mut shown = l.clone();
        let mut name = self.world.program_name(l.prog).to_string();
        if let Some(p) = self.pending.get(&l.id) {
            shown.quota = p.quota.unwrap_or(shown.quota);
            shown.enabled = p.enabled.unwrap_or(shown.enabled);
            name = p.program.clone().unwrap_or(name);
        }
        (shown, name)
    }

    /// Forget what the world now shows (or never will).
    fn settle_pending(&mut self) {
        let now = get_time();
        let base = self.world.base_of(self.me);
        let world = &self.world;
        self.pending.retain(|id, p| {
            let Some(l) = base.and_then(|b| b.line(*id)) else { return false };
            let age = now - p.at;
            let there = p.quota.is_none_or(|q| q == l.quota)
                && p.enabled.is_none_or(|e| e == l.enabled)
                && p.program.as_deref().is_none_or(|n| n == world.program_name(l.prog));
            age < PENDING_MIN || (!there && age < PENDING_MAX)
        });
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
                Event::PlayerJoined { .. }
                    | Event::PlayerEliminated { .. }
                    | Event::BaseDestroyed { .. }
                    | Event::Expanded { .. }
                    | Event::QuestDone { .. }
                    | Event::Shard { level: Some(_), .. }
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
                            Err(e) => *lost = Some(format!("unreadable world: {e}")),
                        },
                        NetEvent::Msg(ServerMsg::Online { players }) => *online = players,
                        NetEvent::Msg(ServerMsg::Refused(m)) => *lost = Some(m),
                        NetEvent::Msg(ServerMsg::Welcome { .. }) => {}
                        NetEvent::Closed(e) => {
                            if lost.is_none() {
                                *lost = Some(format!("Connection lost: {e}"));
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
                    self.toast(format!("{name} arrives in the sector"), player_color(player));
                }
                Event::PlayerEliminated { player } if player == self.me => {
                    self.toast("Your colony is gone.", pal::BAD);
                }
                Event::BaseDestroyed { owner, .. } if owner == self.me => {
                    self.toast("Your base has fallen", pal::BAD);
                }
                Event::Expanded { owner, capacity } if owner == self.me => {
                    self.toast(format!("Hangar enlarged: room for {capacity} dronoids"), pal::GOOD);
                }
                Event::Shard { owner, perk, level: Some(level), .. } if owner == self.me => {
                    let text = format!("{} level {level}: {}", perk.name(), perk.effect(level));
                    self.toast(text, gfx::perk_color(perk));
                }
                Event::QuestDone { player, quest, reward } if player == self.me => {
                    let title = QUESTS.get(quest as usize).map_or("", |q| q.title);
                    let gain = if reward > 0 { format!(" · +{reward} crystals") } else { String::new() };
                    self.toast(format!("Objective reached: {title}{gain}"), pal::ORE);
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
        self.settle_pending();
        self.watch_base();
        if was_landing && !self.landing() {
            // The base has landed: select it.
            if let Some(b) = self.world.base_of(self.me) {
                self.sel = Sel::Base(b.id);
                self.tool = Tool::None;
            }
        }

        if self.follow
            && let Sel::Dronoid(id) = self.sel
            && let Some(p) = WorldView::dronoid_pos(&self.world, id, self.alpha())
        {
            self.cam.center = self.cam.center.lerp(p + self.focus_shift(), (dt * 5.0).min(1.0));
        }
        self.cam.shake = self.view.shake_offset();

        self.sight = Sight::of(&self.world, self.me, self.alpha(), self.cam.sight_area());
        bloom.begin();
        let hl = self.highlight(ui);
        self.view.draw(&self.world, &self.cam, self.alpha(), &hl, &self.sight);
        let (sight, cam) = (&self.sight, &self.cam);
        self.fog.apply(bloom, FOG_UNKNOWN, || sight.paint_mask(cam));
        bloom.finish();

        let compact = ui::compact();
        let mut acts = vec![];
        // Where the space under the status and objective lines begins.
        let mut top = 48.0f32;
        // On a small screen the reference and the editor take the whole
        // screen: nothing under them to see or to touch by mistake.
        let help_alone = compact && self.help_open;
        let editor_alone = compact && self.editor_open;
        // Under a dialog or the reference, nothing answers the pointer.
        let modal = self.help_open || self.dialog_open();
        let pointer = (ui.pressed, ui.mouse);
        if modal {
            ui.pressed = false;
            ui.mouse = ui::NOWHERE;
        }
        if !help_alone {
            if !editor_alone {
                self.status(ui, &mut acts);
                top = self.objective(ui);
                self.dock(ui, &mut acts);
                if self.players_open {
                    self.players_panel(ui, top);
                }
            }
            // Panels and toasts on a small screen pile up from the dock.
            let mut bottom = screen_height() - if compact { 52.0 } else { 16.0 };
            if self.editor_open {
                self.editor_window(ui, lib, top, &mut acts);
            } else {
                if let Some(y) = self.selection_panel(ui, lib, top, &mut acts) {
                    bottom = y;
                }
                if self.map_open {
                    let mm = if compact {
                        let s = (screen_width() * 0.42).min(170.0);
                        Rect::new(screen_width() - s - 8.0, bottom - s - 8.0, s, s)
                    } else {
                        let s = 190.0;
                        Rect::new(screen_width() - s - 12.0, screen_height() - s - 12.0, s, s)
                    };
                    ui.block(mm);
                    if let Some(p) = draw_minimap(&self.world, &self.cam, mm, &self.sight, ui.mouse, ui.pressed) {
                        ui.pressed = false;
                        acts.push(Act::CenterOn(p));
                    }
                }
            }
            self.draw_toasts(if compact { bottom } else { screen_height() - 58.0 });
            if modal {
                (ui.pressed, ui.mouse) = pointer;
            }
            self.overlays(ui, &mut acts);
        }
        if modal {
            (ui.pressed, ui.mouse) = pointer;
        }
        if self.help_open {
            help_overlay(ui, &mut acts, &mut self.help_scroll);
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

    /// Say so when our base is being hit (now and then, not with every
    /// shot): the fight may well be off screen.
    fn watch_base(&mut self) {
        let now = self.world.base_of(self.me).map(|b| b.hp);
        let before = std::mem::replace(&mut self.hull, now);
        if let (Some(now), Some(before)) = (now, before)
            && now < before
            && get_time() - self.warned_at > 20.0
        {
            self.warned_at = get_time();
            self.toast("Your base is under attack", pal::BAD);
        }
    }

    fn validate_selection(&mut self) {
        // Someone else's unit that slips into the fog is no longer selected.
        let lost = match self.sel {
            Sel::Base(id) => self.world.base(id).is_some_and(|b| !self.sight.shows(b.owner, fpv(b.x, b.y))),
            Sel::Dronoid(id) => self.world.dronoid(id).is_some_and(|d| !self.sight.shows(d.owner, fpv(d.x, d.y))),
            Sel::None => false,
        };
        if lost {
            self.sel = Sel::None;
        }
        match self.sel {
            Sel::Base(id) if self.world.base(id).is_none() => self.sel = Sel::None,
            Sel::Dronoid(id) if self.world.dronoid(id).is_none() => {
                self.sel = Sel::None;
                self.follow = false;
            }
            _ => {}
        }
        let my_base = self.world.base_of(self.me).map(|b| b.id);
        if self.program_menu.is_some() && (my_base.is_none() || self.sel != Sel::Base(my_base.unwrap_or(0))) {
            self.program_menu = None;
        }
        if self.tool == Tool::Target && my_base.is_none() {
            self.tool = Tool::None;
        }
    }

    fn mouse_fp(&self, ui: &Ui) -> (i32, i32) {
        let w = self.cam.s2w(ui.mouse);
        ((w.x * FP as f32) as i32, (w.y * FP as f32) as i32)
    }

    fn highlight(&self, ui: &Ui) -> Highlight {
        let ghost = if self.landing() && !ui.over_ui() && !self.editor_open {
            let (x, y) = self.mouse_fp(ui);
            Some((fpv(x, y), self.world.can_land(self.me, x, y).is_ok()))
        } else {
            None
        };
        let mine = self.world.base_of(self.me);
        let target = match (self.tool, self.sel) {
            (Tool::Target, _) if !ui.over_ui() => Some(self.cam.s2w(ui.mouse)),
            (_, Sel::Base(id)) => mine
                .filter(|b| b.id == id && b.mem[2] == 1)
                .map(|b| vec2(b.mem[0] as f32 + 0.5, b.mem[1] as f32 + 0.5)),
            _ => None,
        };
        let landing = self.landing().then(|| {
            let h = self.world.players[self.me as usize].home;
            fpv(h.0, h.1)
        });
        Highlight {
            base: if let Sel::Base(id) = self.sel { Some(id) } else { None },
            dronoid: if let Sel::Dronoid(id) = self.sel { Some(id) } else { None },
            ghost,
            target,
            landing,
            me: self.me,
        }
    }

    fn act(&mut self, a: Act, lib: &mut Library) -> Option<Outcome> {
        match a {
            Act::Issue(c) => self.issue(c),
            Act::OpenProgram(name, line) => self.open_program(lib, &name, line),
            Act::OpenDebug(id) => self.open_debug(id),
            Act::Select(s) => {
                self.sel = s;
                self.program_menu = None;
                if !matches!(s, Sel::Dronoid(_)) {
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
            Act::ToggleProgramMenu(line) => {
                self.program_menu = if self.program_menu == Some(line) { None } else { Some(line) };
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
                        self.open_program(lib, &n, self.edit_line);
                        self.toast(format!("Copy created: {n}"), pal::GOOD);
                    }
                    Err(e) => self.toast(e, pal::BAD),
                }
            }
            Act::ToggleTextMode => {
                self.text_mode = !self.text_mode;
                self.editor.focused = self.text_mode && self.edit_target == EditTarget::Program;
            }
            Act::ResetBuiltin => {
                let name = self.edit_name.text.clone();
                if let Some(src) = Library::builtin_source(&name) {
                    self.editor.set_text(src);
                    self.editor.dirty = true;
                    self.toast("Original version restored (not applied yet)", pal::WARN);
                }
            }
            Act::LoadProgram(name) => {
                if self.editor.dirty {
                    self.toast("Unapplied changes discarded", pal::WARN);
                }
                self.open_program(lib, &name, self.edit_line);
            }
            Act::CenterOn(p) => {
                self.cam.center = p + self.focus_shift();
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

    /// The program running on one of my production lines.
    fn line_program(&self, line: u32) -> Option<u32> {
        self.world.base_of(self.me).and_then(|b| b.line(line)).map(|l| l.prog)
    }

    /// Which of my base's lines this is, counting from 1.
    fn line_number(&self, line: u32) -> Option<usize> {
        self.world.base_of(self.me).and_then(|b| b.lines.iter().position(|l| l.id == line)).map(|i| i + 1)
    }

    fn open_program(&mut self, lib: &Library, name: &str, line: Option<u32>) {
        let src = lib
            .source_or_builtin(name)
            .or_else(|| line.and_then(|l| self.line_program(l)).map(|p| self.world.programs[p as usize].source.clone()))
            .unwrap_or_else(|| programs::TEMPLATE.to_string());
        self.editor.set_text(&src);
        self.blocks = Blocks::default();
        self.editor.read_only = false;
        self.editor.exec_line = None;
        self.edit_name.text = name.to_string();
        self.edit_target = EditTarget::Program;
        self.edit_line = line;
        self.editor_open = true;
        // Typing goes to the code only when it is shown as text.
        self.editor.focused = self.text_mode;
        self.tool = Tool::None;
        // The base whose line this is stays in view beside the editor.
        if let Some(p) = line.and(self.world.base_of(self.me)).map(|b| fpv(b.x, b.y))
            && !ui::compact()
            && self.cam.w2s(p).x > Game::editor_left() - 40.0
        {
            self.cam.center = p + self.focus_shift();
        }
    }

    fn open_debug(&mut self, id: u32) {
        let Some(d) = self.world.dronoid(id) else { return };
        let (prog, line) = (d.prog, d.line);
        self.editor.set_text(&self.world.programs[prog as usize].source);
        self.blocks = Blocks::default();
        self.editor.read_only = true;
        self.edit_name.text = self.world.program_name(prog).to_string();
        self.debug_prog = prog;
        self.edit_target = EditTarget::Debug(id);
        self.edit_line = Some(line);
        self.editor_open = true;
        self.editor.focused = false;
        self.sel = Sel::Dronoid(id);
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
                if let Some(line) = self.edit_line
                    && let Some(prog) = self.line_program(line)
                    && self.world.program_name(prog) != name
                {
                    self.issue(Command::SetProgram { line, name: name.clone(), source: src });
                    extra = format!(", line {}", self.line_number(line).unwrap_or(0));
                }
                self.editor.dirty = false;
                self.toast(format!("{name} applied{extra}"), pal::GOOD);
            }
            Err(errs) => {
                self.toast(format!("Saved, but {} error(s): not applied", errs.len()), pal::BAD);
            }
        }
    }

    /// How many of my production lines run a program of this name.
    fn users_of(&self, name: &str) -> usize {
        self.world.base_of(self.me).map_or(0, |b| b.lines.iter().filter(|l| self.world.program_name(l.prog) == name).count())
    }

    /// Where the editor's window starts, on a screen wide enough to keep
    /// some of the map beside it.
    fn editor_left() -> f32 {
        let sw = screen_width();
        (sw * 0.38).max(sw - 1000.0).min(sw - 560.0).max(10.0)
    }

    /// How far (in world units) the camera aims to the side of what it
    /// looks at, so that it shows in the part of the map the editor leaves
    /// free rather than behind it.
    fn focus_shift(&self) -> Vec2 {
        if !self.editor_open || ui::compact() {
            return Vec2::ZERO;
        }
        let free = Game::editor_left();
        // Too thin a strip to show anything in.
        if free < 160.0 {
            return Vec2::ZERO;
        }
        vec2((screen_width() - free) / 2.0 / self.cam.zoom, 0.0)
    }

    /// Under the code, only what calls for something: the errors, or what
    /// Apply will do once there are changes to apply.
    fn editor_notes(&self) -> Vec<(String, Color)> {
        if matches!(self.edit_target, EditTarget::Debug(_)) {
            return vec![];
        }
        if !self.editor.errors.is_empty() {
            let bad = mix(pal::BAD, WHITE, 0.35);
            let mut notes: Vec<(String, Color)> =
                self.editor.errors.iter().take(2).map(|e| (format!("line {}: {}", e.line + 1, e.msg), bad)).collect();
            let more = self.editor.errors.len().saturating_sub(2);
            if more > 0
                && let Some(last) = notes.last_mut()
            {
                last.0 += &format!("  (+{more} more)");
            }
            return notes;
        }
        if !self.editor.dirty {
            return vec![];
        }
        let users = self.users_of(&self.edit_name.text);
        if users > 0 {
            vec![(format!("Apply updates {users} production line{}", if users == 1 { "" } else { "s" }), pal::DIM)]
        } else if let Some(n) = self.edit_line.and_then(|l| self.line_number(l)) {
            vec![(format!("Apply gives this program to line {n}"), pal::DIM)]
        } else {
            vec![]
        }
    }

    fn editor_layout(&self, top: f32, notes: usize) -> EditorLayout {
        let (sw, sh) = (screen_width(), screen_height());
        let debug = matches!(self.edit_target, EditTarget::Debug(_));
        // The code takes the room the notes do not need.
        let notes = notes as f32;
        if ui::compact() {
            // The whole screen: two rows of buttons, the code, the dronoid's
            // state under it when debugging; the program list folds over the code.
            let win = Rect::new(4.0, 4.0, sw - 8.0, sh - 8.0);
            let header = Rect::new(win.x + 6.0, win.y + 6.0, win.w - 12.0, 32.0);
            let header2 = Rect::new(header.x, header.y + header.h + 6.0, header.w, 32.0);
            let fh = if notes > 0.0 { notes * 18.0 + 8.0 } else { 2.0 };
            let footer = Rect::new(header.x, win.y + win.h - fh - 4.0, header.w, fh);
            let body_y = header2.y + header2.h + 6.0;
            let body_h = footer.y - 4.0 - body_y;
            let side_h = if debug { (body_h * 0.42).clamp(150.0, 230.0) } else { 0.0 };
            let code = Rect::new(header.x, body_y, header.w, body_h - if debug { side_h + 6.0 } else { 0.0 });
            let side = Rect::new(header.x, code.y + code.h + 6.0, header.w, side_h);
            let list = Rect::new(code.x, code.y, (code.w * 0.6).min(240.0), code.h);
            return EditorLayout { win, header, header2: Some(header2), list, code, side, footer };
        }
        let x0 = Game::editor_left();
        let win = Rect::new(x0, top, sw - x0 - 12.0, sh - top - 60.0);
        let header = Rect::new(win.x + 8.0, win.y + 8.0, win.w - 16.0, 30.0);
        let fh = if notes > 0.0 { notes * 19.0 + 10.0 } else { 0.0 };
        let footer = Rect::new(win.x + 8.0, win.y + win.h - fh - 8.0, win.w - 16.0, fh);
        let body_y = header.y + header.h + 8.0;
        let body_h = footer.y - if notes > 0.0 { 8.0 } else { 0.0 } - body_y;
        let list_w = if debug { 0.0 } else { 140.0 };
        let side_w = if debug { 210.0 } else { 0.0 };
        let list = Rect::new(win.x + 8.0, body_y, list_w, body_h);
        let code_x = list.x + list_w + if debug { 0.0 } else { 8.0 };
        let code = Rect::new(code_x, body_y, win.x + win.w - 8.0 - side_w - code_x - if debug { 8.0 } else { 0.0 }, body_h);
        let side = Rect::new(win.x + win.w - 8.0 - side_w, body_y, side_w, body_h);
        EditorLayout { win, header, header2: None, list, code, side, footer }
    }

    /// The player's programs, one row each; touching one loads it.
    /// Returns whether one was picked.
    fn program_list(&self, ui: &mut Ui, lib: &Library, lr: Rect, row: f32, acts: &mut Vec<Act>) -> bool {
        let mut y = lr.y + 4.0;
        let mut picked = false;
        for e in &lib.entries {
            if y + row > lr.y + lr.h {
                break;
            }
            let r = Rect::new(lr.x, y, lr.w, row - 2.0);
            let current = e.name == self.edit_name.text;
            let used = self.users_of(&e.name);
            if current {
                draw_rectangle(r.x, r.y, 2.0, r.h, pal::ACCENT);
            } else if ui.hover(r) {
                draw_rectangle(r.x, r.y, r.w, r.h, with_alpha(pal::ACCENT, 0.06));
            }
            let ty = r.y + (r.h - 13.0) / 2.0;
            gfx::text(&e.name, r.x + 10.0, ty, 13.0, if current { pal::TEXT } else { pal::DIM });
            if used > 0 {
                let s = format!("{used}");
                gfx::text(&s, r.x + r.w - gfx::text_width(&s, 11.0) - 4.0, ty + 1.0, 11.0, with_alpha(pal::GOOD, 0.8));
            }
            if !current && ui.click(r) {
                acts.push(Act::LoadProgram(e.name.clone()));
                picked = true;
            }
            y += row;
        }
        picked
    }

    fn editor_window(&mut self, ui: &mut Ui, lib: &Library, top: f32, acts: &mut Vec<Act>) {
        let lines = self.editor_notes();
        let l = self.editor_layout(top, lines.len());
        // A little smaller on a phone, for longer lines.
        self.editor.font_size = if l.header2.is_some() { 14.0 } else { 16.0 };
        ui.solid_panel(l.win);
        let debug_id = if let EditTarget::Debug(id) = self.edit_target { Some(id) } else { None };

        if let Some(id) = debug_id {
            match self.world.dronoid(id) {
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
        if let Some(h2) = l.header2 {
            // Small screen: the name and the way out on top, actions below.
            let close = Rect::new(h.x + h.w - 34.0, h.y, 34.0, bh);
            if ui.button(close, "×") {
                acts.push(Act::CloseEditor);
            }
            if let Some(id) = debug_id {
                let alive = self.world.dronoid(id).is_some();
                let title = if alive { format!("Dronoid #{id}") } else { format!("Dronoid #{id} (destroyed)") };
                gfx::text(&title, h.x, h.y + 8.0, 16.0, if alive { pal::ACCENT } else { pal::BAD });
                let tx = h.x + gfx::text_width(&title, 16.0) + 12.0;
                let runs = format!("runs {}", self.edit_name.text);
                gfx::text(&runs, tx, h.y + 10.0, gfx::fit_size(&runs, 13.0, close.x - tx - 8.0), pal::DIM);
                if ui.button_ex(Rect::new(h2.x, h2.y, 136.0, h2.h), "Edit the code", false, true, pal::GOOD) {
                    acts.push(Act::OpenProgram(self.edit_name.text.clone(), self.edit_line));
                }
                if !self.is_online() && ui.button_ex(Rect::new(h2.x + 142.0, h2.y, 110.0, h2.h), "Step by step", false, true, pal::WARN) {
                    acts.push(Act::Step);
                }
            } else {
                let help = Rect::new(close.x - 38.0, h.y, 34.0, bh);
                if ui.button(help, "?") {
                    acts.push(Act::ToggleHelp);
                }
                let name_w = help.x - 22.0 - h.x;
                self.edit_name.ui(ui, Rect::new(h.x, h.y, name_w, bh), "name");
                if self.edit_name.focused {
                    self.editor.focused = false;
                }
                if self.editor.dirty {
                    draw_circle(h.x + name_w + 10.0, h.y + bh / 2.0, 4.0, pal::WARN);
                }
                let builtin = Library::is_builtin(&self.edit_name.text);
                let row: [(&str, Option<Act>, bool, Color); 6] = [
                    ("List", None, true, pal::ACCENT),
                    ("Apply", Some(Act::Apply), true, pal::GOOD),
                    (if self.text_mode { "Tiles" } else { "Text" }, Some(Act::ToggleTextMode), true, pal::ACCENT),
                    ("New", Some(Act::NewProgram), true, pal::ACCENT),
                    ("Duplicate", Some(Act::Duplicate), true, pal::ACCENT),
                    ("Original", Some(Act::ResetBuiltin), builtin, pal::ACCENT),
                ];
                let mut bx = h2.x;
                for (label, act, enabled, color) in row {
                    let w = gfx::text_width(label, 13.0) + 22.0;
                    if bx + w > h2.x + h2.w {
                        break;
                    }
                    let active = act.is_none() && self.list_open;
                    if ui.button_ex(Rect::new(bx, h2.y, w, h2.h), label, active, enabled, color) {
                        match act {
                            Some(a) => acts.push(a),
                            None => self.list_open = !self.list_open,
                        }
                    }
                    bx += w + 4.0;
                }
            }
        } else if let Some(id) = debug_id {
            let alive = self.world.dronoid(id).is_some();
            let title = if alive { format!("Dronoid #{id}") } else { format!("Dronoid #{id} (destroyed)") };
            gfx::text(&title, x, h.y + 7.0, 16.0, if alive { pal::ACCENT } else { pal::BAD });
            x += gfx::text_width(&title, 16.0) + 14.0;
            gfx::text(&format!("runs {}", self.edit_name.text), x, h.y + 9.0, 13.0, pal::DIM);
            let mut bx = h.x + h.w - 30.0;
            if ui.button(Rect::new(bx, h.y, 30.0, bh), "×") {
                acts.push(Act::CloseEditor);
            }
            bx -= 128.0;
            if ui.button_ex(Rect::new(bx, h.y, 122.0, bh), "Edit the code", false, true, pal::GOOD) {
                acts.push(Act::OpenProgram(self.edit_name.text.clone(), self.edit_line));
            }
            if !self.is_online() {
                bx -= 128.0;
                if ui.button_ex(Rect::new(bx, h.y, 122.0, bh), "Step by step", false, true, pal::WARN) {
                    acts.push(Act::Step);
                }
            }
        } else {
            self.edit_name.ui(ui, Rect::new(x, h.y, 180.0, bh), "name");
            if self.edit_name.focused {
                self.editor.focused = false;
            }
            x += 186.0;
            if self.editor.dirty {
                draw_circle(x + 6.0, h.y + bh / 2.0, 4.0, pal::WARN);
                if ui.hover(Rect::new(x, h.y, 14.0, bh)) {
                    ui.tooltip("Unapplied changes");
                }
            }
            x += 18.0;
            if ui.button_primary(Rect::new(x, h.y, 128.0, bh), "Apply", pal::GOOD) {
                acts.push(Act::Apply);
            }
            x += 134.0;
            let rest = [
                ("New", Act::NewProgram, true),
                ("Duplicate", Act::Duplicate, true),
                ("Original", Act::ResetBuiltin, Library::is_builtin(&self.edit_name.text)),
            ];
            for (label, act, enabled) in rest {
                let w = gfx::text_width(label, 13.0) + 20.0;
                if x + w > h.x + h.w - 136.0 {
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
            let switch = Rect::new(h.x + h.w - 130.0, h.y, 62.0, bh);
            if ui.button_ex(switch, if self.text_mode { "Tiles" } else { "Text" }, false, true, pal::ACCENT) {
                acts.push(Act::ToggleTextMode);
            }
            if ui.hover(switch) {
                ui.tooltip(if self.text_mode { "Back to the tiles" } else { "The program as text, to type it" });
            }

            self.program_list(ui, lib, Rect::new(l.list.x, l.list.y, l.list.w - 6.0, l.list.h), 24.0, acts);
        }

        // The folded list lies over the code: a touch there is the list's.
        let list_over = (l.header2.is_some() && debug_id.is_none() && self.list_open).then(|| {
            let rows = lib.entries.len() as f32;
            Rect::new(l.list.x, l.list.y, l.list.w, (rows * 32.0 + 10.0).min(l.list.h))
        });
        let list_pressed = list_over.is_some_and(|r| ui.pressed && r.contains(ui.mouse));
        if list_pressed {
            ui.pressed = false;
        }

        if self.text_mode {
            if self.editor.update(ui, l.code) {
                acts.push(Act::Apply);
            }
            if self.editor.focused {
                self.edit_name.focused = false;
            }
        }
        if debug_id.is_some() {
            if self.text_mode
                && let Some(line) = self.editor.exec_line
            {
                self.editor.reveal(l.code, line);
            }
        } else {
            let src = self.editor.text();
            self.editor.errors = match vm::assemble(&self.edit_name.text, &src) {
                Ok(_) => vec![],
                Err(e) => e,
            };
        }
        if self.text_mode {
            self.editor.draw(ui, l.code);
        } else {
            self.editor.focused = false;
            let src = self.editor.text();
            let exec = debug_id.and(self.editor.exec_line);
            if let Some(next) = self.blocks.ui(ui, l.code, &src, &self.editor.errors, exec, debug_id.is_some()) {
                self.editor.set_text(&next);
                self.editor.dirty = true;
            }
            if ui.ctrl && is_key_pressed(KeyCode::S) && debug_id.is_none() {
                acts.push(Act::Apply);
            }
        }
        if let Some(r) = list_over {
            ui.pressed = list_pressed;
            ui.solid_panel(r);
            if self.program_list(ui, lib, Rect::new(r.x + 4.0, r.y + 1.0, r.w - 8.0, r.h - 2.0), 32.0, acts) {
                self.list_open = false;
            }
            ui.pressed = false;
        }

        if let Some(id) = debug_id {
            self.debug_side(ui, l.side, id, acts);
        }

        let f = l.footer;
        let size = if l.header2.is_some() { 12.0 } else { 13.0 };
        for (i, (s, c)) in lines.iter().enumerate() {
            gfx::text(s, f.x + 4.0, f.y + 4.0 + i as f32 * (size + 6.0), gfx::fit_size(s, size, f.w - 8.0), *c);
        }
    }

    fn debug_side(&self, ui: &mut Ui, r: Rect, id: u32, acts: &mut Vec<Act>) {
        let Some(d) = self.world.dronoid(id) else {
            gfx::text("Dronoid destroyed.", r.x + 10.0, r.y + 10.0, 14.0, pal::BAD);
            return;
        };
        // Under the code on a small screen, the panel is wide: two columns.
        let two = r.w > r.h;
        let col_w = if two { r.w / 2.0 } else { r.w };
        let mut y = r.y + 4.0;
        let mut x = r.x + 10.0;
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
        if two {
            x = r.x + col_w + 4.0;
            y = r.y + 4.0;
        }
        let cell_h = if two { 18.0 } else { 22.0 };
        let rows = [
            ("hold", format!("{}/{}", d.cargo, CARGO_CAP)),
            ("heading", format!("{}°", d.heading)),
            ("position", format!("{}, {}", d.x / FP, d.y / FP)),
            ("HP", format!("{}", d.hp)),
        ];
        for (k, v) in rows {
            gfx::text(k, x, y, 12.0, pal::DIM);
            gfx::text(&v, x + 74.0, y, 12.0, pal::TEXT);
            y += 18.0;
        }
        y += 10.0;
        gfx::text("hive memory", x, y, 11.0, pal::FAINT);
        y += 16.0;
        if let Some(b) = self.world.base_of(d.owner) {
            let cw = (col_w - 20.0) / 4.0;
            for (i, &v) in b.mem.iter().enumerate() {
                let cx = x + (i % 4) as f32 * cw;
                let cy = y + (i / 4) as f32 * cell_h;
                gfx::text(&format!("{v}"), cx + 2.0, cy + 2.0, 12.0, if v != 0 { pal::TEXT } else { pal::FAINT });
            }
            y += HIVE_MEM.div_ceil(4) as f32 * cell_h + 6.0;
        }
        let fr = Rect::new(x, y.min(r.y + r.h - 32.0), col_w - 20.0, 26.0);
        if ui.button_ex(fr, if self.follow { "The camera follows it" } else { "Follow this dronoid" }, self.follow, true, pal::ACCENT) {
            acts.push(Act::Follow(!self.follow));
        }
    }

    // ----- HUD --------------------------------------------------------------

    /// Top-left: the crystals (and the newcomer's shield while it lasts, a
    /// connection only when it fails). Top-right: speed, help, menu.
    fn status(&self, ui: &mut Ui, acts: &mut Vec<Act>) {
        let Some(me) = self.world.players.get(self.me as usize) else { return };
        let (x, y) = (16.0, 14.0);
        draw_poly(x + 5.0, y + 9.0, 4, 6.0, 0.0, pal::ORE);
        let mins = me.minerals.to_string();
        gfx::text(&mins, x + 16.0, y, 16.0, pal::ORE);
        let mut lx = x + 16.0 + gfx::text_width(&mins, 16.0) + 14.0;
        let mut note = |s: &str, c: Color| {
            gfx::text(s, lx, y + 3.0, 12.0, c);
            lx += gfx::text_width(s, 12.0) + 12.0;
        };
        // The size of the colony, once there is one (not on a small screen).
        if !ui::compact()
            && let Some(b) = self.world.base_of(self.me)
        {
            let dronoids = self.world.dronoid_count(self.me);
            let full = dronoids >= b.room();
            let crowd = format!("{dronoids}/{} dronoids{}", b.room(), if full { " · hangar full" } else { "" });
            note(&crowd, if full { pal::WARN } else { pal::DIM });
        }
        if self.world.shielded(self.me) {
            let left =(me.shield_until - self.world.tick) as f32 / TICKS_PER_SEC;
            note(&format!("shielded {}:{:02}", left as u32 / 60, left as u32 % 60), with_alpha(pal::ACCENT, 0.6));
        }
        if let Driver::Online { frames, lost, .. } = &self.driver {
            if lost.is_some() {
                note("offline", pal::BAD);
            } else if frames.len() > 8 {
                note("syncing…", pal::WARN);
            }
        }

        let mut rx = screen_width() - 12.0;
        let small = |ui: &mut Ui, rx: &mut f32, label: &str, active: bool| -> bool {
            let w = gfx::text_width(label, 13.0) + 18.0;
            *rx -= w;
            let hit = ui.button_ex(Rect::new(*rx, 10.0, w, 26.0), label, active, true, pal::ACCENT);
            *rx -= 4.0;
            hit
        };
        if small(ui, &mut rx, "Menu", false) {
            acts.push(Act::Menu);
        }
        if small(ui, &mut rx, "?", self.help_open) {
            acts.push(Act::ToggleHelp);
        }
        // Offline, one button walks through the speeds (keys 1-4 and Space too).
        if !self.is_online() {
            const SPEEDS: [f32; 5] = [1.0, 2.0, 4.0, 8.0, 0.0];
            let at = SPEEDS.iter().position(|s| (self.speed - s).abs() < 0.01).unwrap_or(0);
            let label = if self.speed == 0.0 { "II".to_string() } else { format!("x{}", self.speed) };
            if small(ui, &mut rx, &label, self.speed != 1.0) {
                acts.push(Act::Speed(SPEEDS[(at + 1) % SPEEDS.len()]));
            }
        }
    }

    /// One short line of guidance: what the current tool expects, or else
    /// the next first-steps objective and how far along it is. Returns
    /// where the space under it begins.
    fn objective(&self, ui: &Ui) -> f32 {
        let y = 46.0;
        let pulse = with_alpha(pal::TEXT, 0.75 + 0.25 * (get_time() as f32 * 2.0).sin());
        let say = |mouse: &str, finger: &str| -> Vec<(String, Color)> { vec![((if ui.touch { finger } else { mouse }).to_string(), pulse)] };
        let parts: Vec<(String, Color)> = match self.tool {
            Tool::Target => say("Click the target", "Tap the target"),
            Tool::None if self.landing() => say("Click a free spot to land your base", "Tap a free spot to land your base"),
            Tool::None => {
                let Some(i) = quests::current(&self.world, self.me) else { return 48.0 };
                let (done, goal) = quests::progress(&self.world, self.me, i);
                let q = &QUESTS[i];
                let mut parts = vec![
                    (format!("{}/{}  ", i + 1, QUESTS.len()), pal::DIM),
                    (q.title.to_string(), with_alpha(pal::TEXT, 0.85)),
                ];
                if goal > 100 {
                    parts.push((format!("  {} %", done * 100 / goal), pal::ACCENT));
                } else if goal > 1 {
                    parts.push((format!("  {done}/{goal}"), pal::ACCENT));
                }
                // How to go about it, and what it earns.
                parts.push((format!(" · {}", if ui.touch { q.hint_touch } else { q.hint }), pal::DIM));
                if q.reward > 0 {
                    parts.push((format!(" · +{}", q.reward), with_alpha(pal::ORE, 0.8)));
                }
                parts
            }
        };
        let sw = screen_width();
        let lines = gfx::rich_wrapped(&parts, 13.0, sw / 2.0, y + 5.0, sw - 48.0, with_alpha(pal::PANEL, 0.45));
        (y + lines as f32 * 25.0 + 8.0).max(48.0)
    }

    /// Bottom-centre: a handful of quiet buttons (shorter and taller on a
    /// small screen, squeezed to fit if need be).
    fn dock(&self, ui: &mut Ui, acts: &mut Vec<Act>) {
        let Some(me) = self.world.players.get(self.me as usize) else { return };
        let compact = ui::compact();
        let can_grow = self.world.base_of(self.me).is_some_and(|b| b.room() < MAX_DRONOIDS_PER_PLAYER);
        let can = me.minerals >= EXPAND_COST && me.alive && can_grow;
        let items: [(String, bool, bool, Color, Act); 5] = [
            (
                if compact { format!("Expand {EXPAND_COST}") } else { format!("Expand · {EXPAND_COST}") },
                false,
                can,
                pal::GOOD,
                Act::Issue(Command::Expand),
            ),
            (
                (if compact { "Code" } else { "Programs" }).into(),
                self.editor_open,
                true,
                pal::ACCENT,
                Act::OpenProgram(self.default_program_name(), None),
            ),
            ("Map".into(), self.map_open, true, pal::ACCENT, Act::ToggleMap),
            ("Colonies".into(), self.players_open, true, pal::ACCENT, Act::TogglePlayers),
            ("Base".into(), false, !self.landing(), pal::ACCENT, Act::CenterOn(self.home())),
        ];
        let (pad, gap, h) = if compact { (16.0, 4.0, 38.0) } else { (26.0, 6.0, 30.0) };
        let mut widths: Vec<f32> = items.iter().map(|i| gfx::text_width(&i.0, 13.0) + pad).collect();
        let gaps = gap * (items.len() as f32 - 1.0);
        let room = screen_width() - 16.0 - gaps;
        let sum: f32 = widths.iter().sum();
        if sum > room {
            widths.iter_mut().for_each(|w| *w *= room / sum);
        }
        let total = widths.iter().sum::<f32>() + gaps;
        let mut x = screen_width() / 2.0 - total / 2.0;
        let y = screen_height() - h - if compact { 8.0 } else { 12.0 };
        let tips = [
            if self.landing() {
                "Land your base first".to_string()
            } else if !can_grow {
                "The hangar is as large as it gets".to_string()
            } else if can {
                format!("Enlarge the hangar: room for {EXPAND_SLOTS} more dronoids (X)")
            } else {
                format!("{EXPAND_SLOTS} more dronoids in the hangar for {EXPAND_COST} crystals: {} to go", (EXPAND_COST - me.minerals).max(0))
            },
            "Read, write and apply programs (P)".into(),
            "The space you have explored (M); click it to look there".into(),
            "Who lives in the sector, and how many dronoids (L)".into(),
            "Back to your base (H)".into(),
        ];
        for (((label, active, enabled, color, act), w), tip) in items.into_iter().zip(widths).zip(tips) {
            let r = Rect::new(x, y, w, h);
            if !ui.touch && ui.hover(r) {
                ui.tooltip(&tip);
            }
            if ui.button_ex(r, &label, active, enabled, color) {
                acts.push(match act {
                    Act::OpenProgram(..) if self.editor_open => Act::CloseEditor,
                    a => a,
                });
            }
            x += w + gap;
        }
    }

    fn players_panel(&self, ui: &mut Ui, top: f32) {
        let mut rows: Vec<(u8, usize, usize, bool)> = self
            .world
            .players
            .iter()
            .map(|p| (p.id, self.world.base_of(p.id).map_or(0, |b| b.room()), self.world.dronoid_count(p.id), p.alive))
            .collect();
        rows.sort_by_key(|r| (!r.3, std::cmp::Reverse(r.2 * 1000 + r.1), r.0));
        let shown = rows.len().min(14);
        let r = if ui::compact() {
            Rect::new(8.0, top, (screen_width() - 16.0).min(260.0), 30.0 + shown as f32 * 19.0)
        } else {
            Rect::new(16.0, top, 260.0, 30.0 + shown as f32 * 19.0)
        };
        ui.panel(r);
        let title = match &self.driver {
            Driver::Online { world_name, .. } => format!("Sector {world_name}"),
            Driver::Offline { .. } => "Training".into(),
        };
        // What the two numbers on each row count.
        let cols = "dronoids / hangar";
        let cw = gfx::text_width(cols, 11.0);
        gfx::text(cols, r.x + r.w - 10.0 - cw, r.y + 8.0, 11.0, pal::DIM);
        gfx::text(&title, r.x + 10.0, r.y + 7.0, gfx::fit_size(&title, 12.0, r.w - 30.0 - cw), pal::DIM);
        let online: &[u8] = match &self.driver {
            Driver::Online { online, .. } => online,
            _ => &[],
        };
        for (i, (id, f, d, alive)) in rows.iter().take(shown).enumerate() {
            let y = r.y + 26.0 + i as f32 * 19.0;
            let p = &self.world.players[*id as usize];
            let c = if *alive { player_color(*id) } else { pal::FAINT };
            if *id == self.me {
                draw_rectangle(r.x + 4.0, y - 2.0, r.w - 8.0, 18.0, with_alpha(c, 0.08));
            }
            draw_poly(r.x + 15.0, y + 7.0, 6, 4.0, 30.0, c);
            let tc = if *id == self.me { pal::TEXT } else if *alive { mix(c, pal::TEXT, 0.6) } else { pal::FAINT };
            gfx::text(&p.name, r.x + 26.0, y, 13.0, tc);
            if online.contains(id) && *id != self.me {
                draw_circle(r.x + 30.0 + gfx::text_width(&p.name, 13.0) + 4.0, y + 7.0, 2.5, pal::GOOD);
            }
            let info = if *alive { format!("{d} / {f}") } else { "—".into() };
            gfx::text(&info, r.x + r.w - 10.0 - gfx::text_width(&info, 12.0), y + 1.0, 12.0, pal::DIM);
        }
    }

    fn home(&self) -> Vec2 {
        self.world
            .base_of(self.me)
            .map(|b| fpv(b.x, b.y))
            .or_else(|| self.world.players.get(self.me as usize).map(|p| fpv(p.home.0, p.home.1)))
            .unwrap_or(self.cam.center)
    }

    /// The first line's program, when my base is selected.
    fn default_program_name(&self) -> String {
        if let Sel::Base(id) = self.sel
            && let Some(b) = self.world.base_of(self.me).filter(|b| b.id == id)
            && let Some(l) = b.lines.first()
        {
            return self.world.program_name(l.prog).to_string();
        }
        "miner".into()
    }

    /// The first production line of my base, with its program's name.
    fn first_line(&self) -> Option<(u32, String)> {
        let l = self.world.base_of(self.me)?.lines.first()?;
        Some((l.id, self.world.program_name(l.prog).to_string()))
    }

    /// Top right; on a small screen, along the bottom above the dock, and
    /// then returns where its top is.
    fn selection_panel(&self, ui: &mut Ui, lib: &Library, top: f32, acts: &mut Vec<Act>) -> Option<f32> {
        let h = match self.sel {
            Sel::None => return None,
            Sel::Base(id) => match self.world.base(id) {
                Some(b) if b.owner == self.me => Game::base_panel_height(b),
                Some(_) => 44.0,
                None => return None,
            },
            Sel::Dronoid(id) => match self.world.dronoid(id) {
                Some(d) if d.owner == self.me => 76.0,
                Some(_) => 44.0,
                None => return None,
            },
        };
        let compact = ui::compact();
        let (sw, sh) = (screen_width(), screen_height());
        let w = if compact { (sw - 16.0).min(420.0) } else { 310.0 };
        let x = if compact { (sw - w) / 2.0 } else { sw - w - 12.0 };
        let y = if compact { sh - 54.0 - h } else { top };
        self.selection_body(ui, lib, acts, Rect::new(x, y, w, h));
        compact.then_some(y)
    }

    /// What a selected unit is and what can be done with it, nothing more:
    /// its health only once it is hurt, the pause only once it is paused,
    /// the target's clear button only when there is a target.
    fn selection_body(&self, ui: &mut Ui, lib: &Library, acts: &mut Vec<Act>, r: Rect) {
        let (x, y, w) = (r.x, r.y, r.w);
        let health = |hp: i32, max: i32| {
            if hp < max {
                let k = hp as f32 / max as f32;
                thin_bar(x + 12.0, y + 32.0, w - 24.0, k, mix(pal::BAD, pal::GOOD, k));
            }
        };
        let theirs = |owner: u8, what: &str| {
            let pc = player_color(owner);
            gfx::text(&self.world.players[owner as usize].name, x + 12.0, y + 10.0, 15.0, mix(pc, WHITE, 0.2));
            gfx::text(what, x + w - 12.0 - gfx::text_width(what, 12.0), y + 13.0, 12.0, pal::DIM);
        };
        match self.sel {
            Sel::None => {}
            Sel::Base(id) => {
                let Some(b) = self.world.base(id) else { return };
                ui.panel(r);
                health(b.hp, b.max_hp());
                if b.owner != self.me {
                    theirs(b.owner, "base");
                    return;
                }
                self.base_body(ui, lib, acts, r, b);
            }
            Sel::Dronoid(id) => {
                let Some(d) = self.world.dronoid(id) else { return };
                ui.panel(r);
                health(d.hp, self.world.base_of(d.owner).map_or(DRONOID_HP, |b| b.dronoid_hp()));
                let (state, col) = doing_label(d.doing);
                if d.owner != self.me {
                    theirs(d.owner, &state);
                    return;
                }
                gfx::text(&state, x + 12.0, y + 10.0, 15.0, col);
                if d.cargo > 0 {
                    let load = format!("{}/{CARGO_CAP}", d.cargo);
                    gfx::text(&load, x + w - 12.0 - gfx::text_width(&load, 13.0), y + 12.0, 13.0, pal::ORE);
                }
                if ui.button_ex(Rect::new(x + 12.0, y + 40.0, 128.0, 28.0), "Debug", false, true, pal::GOOD) {
                    acts.push(Act::OpenDebug(id));
                }
                if ui.button_ex(Rect::new(x + 146.0, y + 40.0, 132.0, 28.0), "Follow", self.follow, true, pal::ACCENT) {
                    acts.push(Act::Follow(!self.follow));
                }
            }
        }
    }

    /// Rows of my base's panel, top to bottom.
    const BASE_HEAD: f32 = 74.0;
    const LINE_ROW: f32 = 32.0;
    const PERK_ROW: f32 = 64.0;

    fn base_panel_height(b: &dronoids::sim::Base) -> f32 {
        let perks = if b.has_perks() { Game::PERK_ROW } else { 0.0 };
        let more = if b.lines.len() < MAX_LINES { 20.0 } else { 4.0 };
        Game::BASE_HEAD + b.lines.len() as f32 * Game::LINE_ROW + perks + more
    }

    /// My base: its hangar, the target of its hunters, then one row per
    /// production line (program, dronoids out of its quota, code, pause),
    /// and what relics gave it.
    fn base_body(&self, ui: &mut Ui, lib: &Library, acts: &mut Vec<Act>, r: Rect, b: &dronoids::sim::Base) {
        let (x, y, w) = (r.x, r.y, r.w);
        let compact = ui::compact();
        let me = &self.world.players[self.me as usize];
        let out = self.world.dronoid_count(self.me);
        gfx::text("base", x + 12.0, y + 10.0, 15.0, pal::ACCENT);
        let hangar = format!("{out}/{} dronoids", b.room());
        let full = out >= b.room();
        gfx::text(&hangar, x + w - 12.0 - gfx::text_width(&hangar, 12.0), y + 12.0, 12.0, if full { pal::WARN } else { pal::DIM });
        if b.hp < b.max_hp() && ui.hover(Rect::new(x, y + 26.0, w, 12.0)) {
            ui.tooltip(&format!("{} / {}: a damaged base slowly mends itself", b.hp, b.max_hp()));
        }

        let cy = y + 40.0;
        let room = b.room() < MAX_DRONOIDS_PER_PLAYER;
        let expand = Rect::new(x + 12.0, cy, 128.0, 26.0);
        if ui.button_ex(expand, &format!("Expand · {EXPAND_COST}"), false, room && me.minerals >= EXPAND_COST, pal::GOOD) {
            acts.push(Act::Issue(Command::Expand));
        }
        if ui.hover(expand) {
            ui.tooltip(&format!("{EXPAND_SLOTS} more dronoids in the hangar, for {EXPAND_COST} crystals"));
        }
        let aim = Rect::new(x + 146.0, cy, 70.0, 26.0);
        if ui.button_ex(aim, "Target", self.tool == Tool::Target, true, pal::PINK) {
            acts.push(Act::Tool(Tool::Target));
        }
        if ui.hover(aim) {
            ui.tooltip("Writes X, Y to the base's memory 0 and 1 (2 = 1): hunter heads there");
        }
        if b.mem[2] != 0 && ui.button_ex(Rect::new(x + 220.0, cy, 24.0, 26.0), "×", false, true, pal::PINK) {
            acts.push(Act::Issue(Command::SetMemory { index: 2, value: 0 }));
        }

        let wait = b.spawn_ticks();
        let mut ly = y + Game::BASE_HEAD;
        let mut menu_at = None;
        for (i, l) in b.lines.iter().enumerate() {
            let row = Rect::new(x + 8.0, ly, w - 16.0, Game::LINE_ROW - 4.0);
            // As asked, even before the world has it.
            let (l, pname) = self.shown_line(l);
            let (l, pname) = (&l, pname.as_str());
            let n = self.world.dronoids_of_line(l.id);
            // From the right: pause, code, +, −, the count; the program takes the rest.
            let bh = row.h;
            let pause = Rect::new(row.x + row.w - 28.0, row.y, 28.0, bh);
            let code = Rect::new(pause.x - 50.0, row.y, 46.0, bh);
            let plus = Rect::new(code.x - 28.0, row.y, 24.0, bh);
            let minus = Rect::new(plus.x - 26.0, row.y, 24.0, bh);
            let count = format!("{n}/{}", l.quota);
            let cw = gfx::text_width("00/00", 12.0) + 6.0;
            let name = Rect::new(row.x + 18.0, row.y, minus.x - cw - row.x - 22.0, bh);
            let col = if l.enabled { pal::ACCENT } else { pal::DIM };
            gfx::text(&format!("{}", i + 1), row.x + 2.0, row.y + 7.0, 13.0, with_alpha(col, 0.8));
            let open = self.program_menu == Some(l.id);
            if ui.button_ex(name, pname, open, true, if l.enabled { pal::ACCENT } else { pal::WARN }) {
                acts.push(Act::ToggleProgramMenu(l.id));
            }
            if open {
                menu_at = Some((l.id, pname.to_string(), ly));
            }
            gfx::text(&count, minus.x - cw, row.y + 7.0, gfx::fit_size(&count, 12.0, cw - 4.0), pal::DIM);
            if ui.button(minus, "−") && l.quota > 0 {
                acts.push(Act::Issue(Command::SetQuota { line: l.id, quota: l.quota - 1 }));
            }
            if ui.button(plus, "+") && l.quota < MAX_QUOTA {
                acts.push(Act::Issue(Command::SetQuota { line: l.id, quota: l.quota + 1 }));
            }
            if ui.button_ex(code, "Code", false, true, pal::GOOD) {
                acts.push(Act::OpenProgram(pname.to_string(), Some(l.id)));
            }
            if ui.button_ex(pause, if l.enabled { "II" } else { ">" }, !l.enabled, true, pal::WARN) {
                acts.push(Act::Issue(Command::ToggleLine { line: l.id }));
            }
            // What the line is up to: the next dronoid, or why none comes.
            let (why, frac) = if !l.enabled {
                ("paused".to_string(), 0.0)
            } else if n >= l.quota as usize {
                ("full: its quota is reached".to_string(), 1.0)
            } else if out >= b.room() {
                ("the hangar is full: Expand".to_string(), 1.0)
            } else if me.minerals < DRONOID_COST {
                (format!("needs {DRONOID_COST} crystals"), 0.0)
            } else {
                let secs = (l.timer as f32 / TICKS_PER_SEC).ceil() as i32;
                (format!("next dronoid in {secs} s"), 1.0 - l.timer as f32 / wait as f32)
            };
            draw_rectangle(name.x, row.y + bh + 1.0, name.w * frac.clamp(0.0, 1.0), 2.0, with_alpha(col, 0.45));
            if ui.hover(name) {
                ui.tooltip(&format!("line {}: {pname} · {why} · {}", i + 1, if open { "pick a program" } else { "click to change its program" }));
            } else if ui.hover(Rect::new(minus.x - cw, row.y, cw, bh)) {
                ui.tooltip(&format!("{n} dronoids out of the {} this line may keep", l.quota));
            } else if ui.hover(pause) {
                ui.tooltip(if l.enabled { "Pause this line" } else { "Resume this line" });
            }
            ly += Game::LINE_ROW;
        }
        if b.has_perks() {
            self.perk_row(ui, b, Rect::new(x + 12.0, ly + 4.0, w - 24.0, Game::PERK_ROW - 8.0));
            ly += Game::PERK_ROW;
        }
        if b.lines.len() < MAX_LINES {
            let more = format!("Assembly relics add production lines ({}/{MAX_LINES})", b.lines.len());
            gfx::text(&more, x + 12.0, ly + 2.0, gfx::fit_size(&more, 11.0, w - 24.0), pal::FAINT);
        }

        if let Some((line, pname, ry)) = menu_at {
            // Beside the panel, or above it in two columns on a small screen.
            let n = lib.entries.len().min(14);
            let (cols, rh) = if compact { (2, 32.0) } else { (1, 26.0) };
            let rows = n.div_ceil(cols);
            let ph = 10.0 + rows as f32 * rh;
            let pr = if compact { Rect::new(x, y - ph - 6.0, w, ph) } else { Rect::new(x - 172.0, ry, 164.0, ph) };
            ui.panel(pr);
            let cw = (pr.w - 10.0 - 6.0 * (cols - 1) as f32) / cols as f32;
            for (i, e) in lib.entries.iter().take(n).enumerate() {
                let (col, row) = ((i % cols) as f32, (i / cols) as f32);
                let rr = Rect::new(pr.x + 5.0 + col * (cw + 6.0), pr.y + 5.0 + row * rh, cw, rh - 2.0);
                if ui.button_ex(rr, &e.name, e.name == pname, true, pal::ACCENT) {
                    match vm::assemble(&e.name, &e.source) {
                        Ok(_) => acts.push(Act::Issue(Command::SetProgram { line, name: e.name.clone(), source: e.source.clone() })),
                        Err(_) => acts.push(Act::OpenProgram(e.name.clone(), Some(line))),
                    }
                    acts.push(Act::ToggleProgramMenu(line));
                }
                if ui.hover(rr)
                    && let Some(b) = programs::builtin(&e.name)
                {
                    ui.tooltip(b.summary);
                }
            }
        }
    }

    /// What relics gave the base: two rows of five cells, a perk's name
    /// and level over a bar of its hundred levels, the next one filling up
    /// shard by shard just under it.
    fn perk_row(&self, ui: &mut Ui, b: &dronoids::sim::Base, r: Rect) {
        let per_row = PERKS.div_ceil(2);
        let cell = r.w / per_row as f32;
        let row_h = r.h / 2.0;
        for p in Perk::ALL {
            let (col_i, row_i) = (p.index() % per_row, p.index() / per_row);
            let cr = Rect::new(r.x + col_i as f32 * cell, r.y + row_i as f32 * row_h, cell - 6.0, row_h - 4.0);
            let (shards, level) = (b.shards(p), b.level(p));
            let col = if shards > 0 { gfx::perk_color(p) } else { pal::FAINT };
            let tc = if shards > 0 { mix(col, WHITE, 0.3) } else { pal::DIM };
            let l = if level > 0 { format!("{level}") } else { String::new() };
            let lw = if level > 0 { gfx::text_width(&l, 10.0) + 4.0 } else { 0.0 };
            gfx::text(p.name(), cr.x, cr.y, gfx::fit_size(p.name(), 11.0, cr.w - lw), tc);
            gfx::text(&l, cr.x + cr.w - lw + 4.0, cr.y + 1.0, 10.0, col);
            let k = level as f32 / PERK_MAX_LEVEL as f32;
            draw_rectangle(cr.x, cr.y + 16.0, cr.w, 4.0, with_alpha(pal::FAINT, 0.7));
            draw_rectangle(cr.x, cr.y + 16.0, cr.w * k, 4.0, col);
            if level < PERK_MAX_LEVEL {
                let next = (shards - level * SHARDS_PER_LEVEL) as f32 / SHARDS_PER_LEVEL as f32;
                draw_rectangle(cr.x, cr.y + 22.0, cr.w * next, 2.0, with_alpha(col, 0.5));
            }
            if ui.hover(cr) {
                let next = if level >= PERK_MAX_LEVEL {
                    "as high as it goes".to_string()
                } else {
                    format!("{}/{SHARDS_PER_LEVEL} shards to level {}", shards - level * SHARDS_PER_LEVEL, level + 1)
                };
                let now = if level > 0 { format!("level {level}/{PERK_MAX_LEVEL}, {}", p.effect(level)) } else { format!("one level: {}", p.effect(1)) };
                ui.tooltip(&format!("{}: {now} · {next}", p.name()));
            }
        }
    }

    /// The relic under a point of the map, if we know of it.
    fn relic_at(&self, wp: Vec2) -> Option<dronoids::sim::RelicSpot> {
        let (x, y) = ((wp.x * FP as f32) as i32, (wp.y * FP as f32) as i32);
        let mut found = None;
        self.world.relics_near(x, y, 2 * FP, |k| {
            let c = fpv(k.x, k.y);
            if c.distance(wp) < fp(RELIC_RADIUS) * 1.6 && self.sight.explored(c) {
                found = Some(k);
            }
        });
        found
    }

    fn relic_text(k: &dronoids::sim::RelicSpot) -> String {
        format!(
            "{} relic, {} shard{} left · MINE it, {SHARDS_PER_LEVEL} shards a level: {} (prospector looks for them)",
            k.perk.name(),
            k.left,
            if k.left == 1 { "" } else { "s" },
            k.perk.effect(1)
        )
    }

    /// Recent messages, piled up from `y`.
    fn draw_toasts(&self, mut y: f32) {
        for t in self.toasts.iter().rev() {
            let a = (1.0 - ((t.t - 3.2) / 0.8).max(0.0)).clamp(0.0, 1.0) * (t.t * 5.0).min(1.0);
            let size = gfx::fit_size(&t.text, 13.0, screen_width() - 44.0);
            let w = gfx::text_width(&t.text, size) + 28.0;
            let r = Rect::new(screen_width() / 2.0 - w / 2.0, y - 28.0, w, 24.0);
            draw_rectangle(r.x, r.y, r.w, r.h, with_alpha(pal::PANEL, 0.75 * a));
            draw_circle(r.x + 10.0, r.y + 12.0, 2.5, with_alpha(t.color, a));
            gfx::text(&t.text, r.x + 18.0, r.y + 5.0, size, with_alpha(mix(t.color, pal::TEXT, 0.6), a));
            y -= 28.0;
        }
    }

    /// Is one of the dialogs of `overlays` up?
    fn dialog_open(&self) -> bool {
        let lost = matches!(&self.driver, Driver::Online { lost: Some(_), .. });
        let alive = self.world.players.get(self.me as usize).is_some_and(|p| p.alive);
        let won = !self.is_online() && !self.over_dismissed && self.world.winner() == Some(self.me);
        lost || !alive || won
    }

    fn overlays(&self, ui: &mut Ui, acts: &mut Vec<Act>) {
        let (sw, sh) = (screen_width(), screen_height());
        if self.dialog_open() {
            ui.scrim();
        }
        // Dialogs never wider than the screen.
        let dialog = |w: f32, h: f32| -> Rect {
            let w = w.min(sw - 24.0);
            Rect::new(sw / 2.0 - w / 2.0, sh / 2.0 - h / 2.0, w, h)
        };
        if let Driver::Online { lost: Some(msg), .. } = &self.driver {
            let r = dialog(460.0, 140.0);
            ui.solid_panel(r);
            gfx::text_centered("Connection interrupted", sw / 2.0, r.y + 20.0, 18.0, pal::WARN);
            let size = gfx::fit_size(msg, 13.0, r.w - 20.0);
            gfx::text_centered(msg, sw / 2.0, r.y + 54.0, size, pal::DIM);
            if ui.button(Rect::new(sw / 2.0 - 70.0, r.y + 92.0, 140.0, 30.0), "Back to the menu") {
                acts.push(Act::Menu);
            }
            return;
        }
        let me_alive = self.world.players.get(self.me as usize).is_some_and(|p| p.alive);
        if !me_alive {
            let r = dialog(440.0, 160.0);
            ui.solid_panel(r);
            gfx::text_centered("Your colony is gone", sw / 2.0, r.y + 22.0, 20.0, pal::BAD);
            let line = "The sector goes on without you… for now.";
            gfx::text_centered(line, sw / 2.0, r.y + 60.0, gfx::fit_size(line, 13.0, r.w - 20.0), pal::DIM);
            if ui.button_primary(Rect::new(sw / 2.0 - 150.0, r.y + 104.0, 144.0, 32.0), "Start again", pal::GOOD) {
                acts.push(Act::Respawn);
            }
            if ui.button(Rect::new(sw / 2.0 + 6.0, r.y + 104.0, 144.0, 32.0), "Menu") {
                acts.push(Act::Menu);
            }
        }
        // Offline practice against colonies can be won.
        if !self.is_online() && me_alive && !self.over_dismissed && self.world.winner() == Some(self.me) {
            let r = dialog(400.0, 140.0);
            ui.solid_panel(r);
            gfx::text_centered("Sector conquered", sw / 2.0, r.y + 22.0, 22.0, pal::GOOD);
            if ui.button(Rect::new(sw / 2.0 - 150.0, r.y + 90.0, 144.0, 30.0), "Continue") {
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

        // Pointing at a relic says what it is.
        if !over
            && !ui.touch
            && self.tool == Tool::None
            && let Some(k) = self.relic_at(self.cam.s2w(ui.mouse))
        {
            ui.tooltip(&Game::relic_text(&k));
        }

        if !over && ui.wheel != 0.0 {
            let before = self.cam.s2w(ui.mouse);
            self.cam.zoom = (self.cam.zoom * 1.15f32.powf(ui.wheel.signum())).clamp(MIN_ZOOM, MAX_ZOOM);
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
        // Fingers that landed on the map: one slides it, two pinch to zoom
        // around their midpoint, and a tap clicks.
        let mut tapped = false;
        if ui.touch {
            if ui.pressed && !over {
                ui.pressed = false;
                self.touch_world = true;
            }
            if self.touch_world {
                if let Some(p) = ui.pinch {
                    let before = self.cam.s2w(p.mid - p.moved);
                    self.cam.zoom = (self.cam.zoom * p.scale).clamp(MIN_ZOOM, MAX_ZOOM);
                    let after = self.cam.s2w(p.mid);
                    self.cam.center += before - after;
                    self.follow = false;
                } else if let Some(d) = ui.drag {
                    self.cam.center -= d / self.cam.zoom;
                    if d != Vec2::ZERO {
                        self.follow = false;
                    }
                }
                tapped = ui.tap;
            }
            if !ui.down && ui.pinch.is_none() {
                self.touch_world = false;
            }
        }

        let limit = fp(WORLD_EXTENT);
        self.cam.center = self.cam.center.clamp(Vec2::splat(-limit), Vec2::splat(limit));

        if ui.touch {
            if !tapped {
                return;
            }
        } else {
            if !ui.pressed || over {
                return;
            }
            ui.pressed = false;
        }
        let (mx, my) = self.mouse_fp(ui);
        if self.landing() {
            match self.world.can_land(self.me, mx, my) {
                Ok(()) => {
                    let name = "miner".to_string();
                    let source = lib.source_or_builtin(&name).unwrap_or_else(|| programs::TEMPLATE.into());
                    acts.push(Act::Issue(Command::Land { x: mx, y: my, name, source }));
                }
                Err(why) => self.toast(format!("Not here: {why}"), pal::WARN),
            }
            return;
        }
        if self.tool == Tool::Target {
            for (i, v) in [(0u8, mx.div_euclid(FP)), (1, my.div_euclid(FP)), (2, 1)] {
                acts.push(Act::Issue(Command::SetMemory { index: i, value: v }));
            }
            self.tool = Tool::None;
            return;
        }
        let wp = self.cam.s2w(ui.mouse);
        let alpha = self.alpha();
        let pick = (0.9f32).max(10.0 / self.cam.zoom);
        let dronoid = self
            .world
            .dronoids
            .iter()
            .filter_map(|d| WorldView::dronoid_pos(&self.world, d.id, alpha).map(|p| (d.id, p.distance(wp))))
            .filter(|(id, dist)| *dist < pick && self.world.dronoid(*id).is_some_and(|d| self.sight.shows(d.owner, fpv(d.x, d.y))))
            .min_by(|a, b| a.1.total_cmp(&b.1));
        let base = self
            .world
            .bases
            .iter()
            .map(|b| (b.id, fpv(b.x, b.y).distance(wp)))
            .filter(|(id, d)| *d < fp(BASE_RADIUS) + 0.4 && self.world.base(*id).is_some_and(|b| self.sight.shows(b.owner, fpv(b.x, b.y))))
            .min_by(|a, b| a.1.total_cmp(&b.1));
        let sel = match (dronoid, base) {
            (_, Some((id, _))) if dronoid.is_none_or(|d| d.1 > 0.5) => Sel::Base(id),
            (Some((id, _)), _) => Sel::Dronoid(id),
            _ => Sel::None,
        };
        if ui.double_click
            && sel == self.sel
            && let Sel::Base(id) = sel
            && self.world.base_of(self.me).is_some_and(|b| b.id == id)
            && let Some((line, name)) = self.first_line()
        {
            acts.push(Act::OpenProgram(name, Some(line)));
        }
        // A finger has no hover: tapping a relic says what it is.
        if ui.touch
            && sel == Sel::None
            && let Some(k) = self.relic_at(wp)
        {
            self.toast(Game::relic_text(&k), gfx::perk_color(k.perk));
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
        if is_key_pressed(KeyCode::X) && !self.landing() {
            acts.push(Act::Issue(Command::Expand));
        }
        if is_key_pressed(KeyCode::M) {
            acts.push(Act::ToggleMap);
        }
        if is_key_pressed(KeyCode::L) {
            acts.push(Act::TogglePlayers);
        }
        if is_key_pressed(KeyCode::T) && self.world.base_of(self.me).is_some() {
            acts.push(Act::Tool(Tool::Target));
        }
        if is_key_pressed(KeyCode::P) {
            let name = self.default_program_name();
            acts.push(if self.editor_open { Act::CloseEditor } else { Act::OpenProgram(name, None) });
        }
        if is_key_pressed(KeyCode::E) {
            match self.sel {
                Sel::Base(id) => {
                    if self.world.base_of(self.me).is_some_and(|b| b.id == id)
                        && let Some((line, name)) = self.first_line()
                    {
                        acts.push(Act::OpenProgram(name, Some(line)));
                    }
                }
                Sel::Dronoid(id) => {
                    if self.world.dronoid(id).is_some_and(|d| d.owner == self.me) {
                        acts.push(Act::OpenDebug(id));
                    }
                }
                Sel::None => {}
            }
        }
        if is_key_pressed(KeyCode::H) || is_key_pressed(KeyCode::Home) {
            acts.push(Act::CenterOn(self.home()));
        }
        if is_key_pressed(KeyCode::Tab)
            && let Some(b) = self.world.base_of(self.me)
        {
            acts.push(Act::Select(Sel::Base(b.id)));
            acts.push(Act::CenterOn(fpv(b.x, b.y)));
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
        let m = programs::builtin("miner").unwrap();
        if let Some(p) = (0..10)
            .flat_map(|r| (0..360).step_by(30).map(move |a| (r, a)))
            .map(|(r, a)| {
                let (dx, dy) = dronoids::geo::dir_vec(a, r * 3 * FP);
                (hx + dx, hy + dy)
            })
            .find(|p| self.world.can_land(self.me, p.0, p.1).is_ok())
        {
            self.issue(Command::Land { x: p.0, y: p.1, name: m.name.into(), source: m.source.into() });
            self.fast_forward(1);
            self.cam.center = fpv(p.0, p.1);
            self.sel = self.world.base_of(self.me).map(|b| Sel::Base(b.id)).unwrap_or(Sel::None);
        }
    }

    pub fn open_program_by_name(&mut self, lib: &Library, name: &str) {
        self.sel = self.world.base_of(self.me).map(|b| Sel::Base(b.id)).unwrap_or(Sel::None);
        let line = self.first_line().map(|l| l.0);
        self.open_program(lib, name, line);
    }

    pub fn set_overlays(&mut self, help: bool, map: bool, players: bool) {
        self.help_open = help;
        self.map_open = map;
        self.players_open = players;
    }

    pub fn debug_first_dronoid(&mut self) {
        if let Some(id) = self.world.dronoids.iter().find(|d| d.owner == self.me).map(|d| d.id) {
            self.open_debug(id);
        }
    }

    /// Screenshots of perks: the base gets some of each, and with `look` a
    /// dronoid is put beside the nearest relic, with the camera on it.
    pub fn demo_relics(&mut self, look: bool) {
        let Some(bi) = self.world.bases.iter().position(|b| b.owner == self.me) else { return };
        let (fx, fy) = (self.world.bases[bi].x, self.world.bases[bi].y);
        self.world.bases[bi].perks = [140, 60, 33, 260, 5, 70, 400, 18, 52, 0];
        self.world.bases[bi].hp = self.world.bases[bi].max_hp();
        if !look {
            return;
        }
        let mut best: Option<dronoids::sim::RelicSpot> = None;
        let d2 = |k: &dronoids::sim::RelicSpot| fpv(k.x, k.y).distance_squared(fpv(fx, fy));
        self.world.relics_near(fx, fy, 600 * FP, |k| {
            if best.is_none_or(|b| d2(&k) < d2(&b)) {
                best = Some(k);
            }
        });
        if let (Some(k), Some(d)) = (best, self.world.dronoids.iter_mut().find(|d| d.owner == self.me)) {
            (d.x, d.y, d.prev_x, d.prev_y) = (k.x + 3 * FP, k.y + FP, k.x + 3 * FP, k.y + FP);
            self.cam.center = fpv(k.x, k.y);
            self.sel = Sel::None;
        }
    }

    pub fn set_zoom(&mut self, z: f32) {
        self.cam.zoom = z;
    }
}

pub fn bot_name(i: usize) -> String {
    const NAMES: &[&str] = &["Orion", "Vega", "Lyra", "Rigel", "Deneb", "Sirius", "Altair", "Antares"];
    format!("AI {}", NAMES[i % NAMES.len()])
}

fn doing_label(d: Doing) -> (String, Color) {
    match d {
        Doing::Idle => ("thinking".into(), pal::TEXT),
        Doing::Move { .. } => ("moving".into(), pal::TEXT),
        Doing::Mine { .. } => ("mining".into(), pal::ORE),
        Doing::Drop { .. } => ("unloading".into(), pal::ORE),
        Doing::Attack { .. } => ("firing".into(), pal::BAD),
        Doing::Wait => ("waiting".into(), pal::DIM),
        Doing::Fail => ("blocked".into(), pal::WARN),
        Doing::Stall => ("computing…".into(), pal::WARN),
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
fn help_overlay(ui: &mut Ui, acts: &mut Vec<Act>, scroll: &mut f32) {
    if ui::compact() {
        help_scrolling(ui, acts, scroll);
        return;
    }
    let (sw, sh) = (screen_width(), screen_height());
    let w = (sw - 40.0).min(1240.0);
    let h = (sh - 40.0).min(820.0);
    let r = Rect::new((sw - w) / 2.0, (sh - h) / 2.0, w, h);
    // A click beside the reference puts it away.
    if ui.scrim() && !r.contains(ui.mouse) {
        ui.pressed = false;
        acts.push(Act::ToggleHelp);
    }
    ui.solid_panel(r);
    if ui.button(Rect::new(r.x + r.w - 40.0, r.y + 10.0, 30.0, 28.0), "×") {
        acts.push(Act::ToggleHelp);
    }
    gfx::text("DASM, the dronoids' language", r.x + 20.0, r.y + 16.0, 18.0, pal::ACCENT);
    gfx::text(HELP_INTRO, r.x + 20.0, r.y + 42.0, gfx::fit_size(HELP_INTRO, 13.0, w - 80.0), pal::DIM);

    // Two columns of about the same height, so that the text can be as
    // large as the window allows: what a dronoid does, senses and where
    // it jumps on the left; data and the rest of the reference on the right.
    let sections = help_sections(ui.touch);
    let rows = |cat: Cat| vm::MNEMONICS.iter().filter(|m| m.cat == cat).count() as f32;
    let (title_h, row_h) = (24.0, 18.5);
    let left = [Cat::Action, Cat::Sense, Cat::Flow];
    let need_left: f32 = left.iter().map(|c| title_h + rows(*c) * row_h).sum();
    let need_right: f32 =
        title_h + rows(Cat::Data) * row_h + sections.iter().map(|(_, l)| title_h + l.len() as f32 * row_h).sum::<f32>();
    let k = ((h - 90.0) / need_left.max(need_right)).clamp(0.6, 1.0);
    let fs = 13.5 * k;
    let lh = row_h * k;
    let col_w = (w - 60.0) / 2.0;
    let mnemonics = |cat: Cat, x: f32, y: &mut f32| {
        let title = HELP_CATS.iter().find(|c| c.0 == cat).map_or("", |c| c.1);
        gfx::text(title, x, *y, 12.0 * k, with_alpha(token_color(vm::TokKind::Op(cat)), 0.8));
        *y += 18.0 * k;
        for m in vm::MNEMONICS.iter().filter(|m| m.cat == cat) {
            gfx::text(m.syntax, x + 8.0, *y, fs, token_color(vm::TokKind::Op(cat)));
            let hx = x + 160.0 * k;
            gfx::text(m.help, hx, *y, gfx::fit_size(m.help, fs, x + col_w - hx), pal::TEXT);
            *y += lh;
        }
        *y += 6.0 * k;
    };

    let x = r.x + 20.0;
    let mut y = r.y + 72.0;
    for cat in left {
        mnemonics(cat, x, &mut y);
    }

    let x2 = r.x + 40.0 + col_w;
    let mut y = r.y + 72.0;
    mnemonics(Cat::Data, x2, &mut y);
    for (title, lines) in sections {
        gfx::text(title, x2, y, 12.0 * k, with_alpha(pal::VIOLET, 0.85));
        y += 18.0 * k;
        for l in lines {
            gfx::text(&l, x2 + 8.0, y, gfx::fit_size(&l, fs, col_w - 8.0), pal::TEXT);
            y += lh;
        }
        y += 6.0 * k;
    }
}

const HELP_INTRO: &str = "Drag tiles into the program (Text: type it instead). A dronoid runs its instructions up to an ACTION, which takes time; then it goes on.";

const HELP_CATS: [(Cat, &str); 4] = [
    (Cat::Action, "actions · take time, set ok to 1 or 0"),
    (Cat::Sense, "sensors · instant"),
    (Cat::Data, "data"),
    (Cat::Flow, "jumps"),
];

/// The reference beside the instructions; the last part depends on
/// whether the game is played with a mouse or with fingers.
fn help_sections(touch: bool) -> Vec<(&'static str, Vec<String>)> {
    let lines = |l: &[&str]| l.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let mut out = vec![
        ("registers", lines(&[
            "a b c d   the dronoid's free registers",
            "ok        1 if the last action succeeded, else 0",
            "values: register, number (-3, 42) or constant",
        ])),
        ("angles · degrees, clockwise", lines(&[
            "N=0  E=90  S=180  W=270  NONE=-1",
            "ADD a, 90 = turn right; MOD a, 360 to stay within 0..359",
            "x grows eastwards, y southwards (POSX, POSY, DX, DY, ANGLE)",
        ])),
        ("things · SCAN, DIST, DX, DY, COUNT, LOOK", vec![
            "EMPTY=0  WALL=1  ORE=2  ALLY=3  ENEMY=4  BASE=5  EBASE=6  RELIC=7".into(),
            format!("sensors: {} units; EBASE: {}; BASE: anywhere", SENSOR_RANGE / FP, EBASE_RANGE / FP),
            "DIST measures the gap between the two hulls (0 = touching)".into(),
            format!("FULL = {CARGO_CAP} (hold capacity)"),
        ]),
        ("ranges", lines(&[
            "MINE / DROP: DIST ≤ 1     ATTACK: DIST ≤ 2",
            "one MOVE = 1 unit; asteroids, crystals and bases block",
        ])),
        ("hive memory", vec![
            format!("{HIVE_MEM} cells shared by all the base's dronoids (PEEK / POKE)"),
            "Target writes 0 = X, 1 = Y, 2 = 1".into(),
        ]),
        ("relics · far from the starting places", vec![
            format!("MINE within reach of a RELIC takes 1 of its {RELIC_SHARDS} shards"),
            format!("{SHARDS_PER_LEVEL} shards of a kind: a level of that perk for the base (up to {PERK_MAX_LEVEL})"),
            format!("Forge: faster lines · Assembly: a line every {ASSEMBLY_LEVELS} levels"),
            "Refinery: more per unload · Bulwark: more hull · Arsenal: harder hits".into(),
            "Vision: the base sees further · Drills: faster mining".into(),
            "Armor: tougher dronoids · Hangar: more room · Nanites: faster mending".into(),
        ]),
    ];
    if touch {
        out.push(("by touch", lines(&[
            "one finger sliding: move the map; two fingers: zoom",
            "tap: pick the base or a dronoid; twice on the base: its first line's program",
            "editor: slide to scroll, tap to place the cursor and type",
        ])));
    } else {
        out.push(("shortcuts", lines(&[
            "X expand · T target · E program/debugger · P programs",
            "M map · L colonies · H or Tab base",
            "ZQSD/WASD/arrows, right-drag: camera; wheel: zoom",
            "editor: Ctrl+S apply, Ctrl+Z/Y, Ctrl+D, Tab",
        ])));
    }
    out
}

/// The reference on a small screen: everything in one column, which a
/// finger (or the wheel) scrolls.
fn help_scrolling(ui: &mut Ui, acts: &mut Vec<Act>, scroll: &mut f32) {
    let (sw, sh) = (screen_width(), screen_height());
    let r = Rect::new(4.0, 4.0, sw - 8.0, sh - 8.0);
    ui.solid_panel(r);
    if ui.button(Rect::new(r.x + r.w - 42.0, r.y + 8.0, 34.0, 32.0), "×") {
        acts.push(Act::ToggleHelp);
    }
    let title = "DASM, the dronoids' language";
    gfx::text(title, r.x + 14.0, r.y + 16.0, gfx::fit_size(title, 18.0, r.w - 70.0), pal::ACCENT);
    let body = Rect::new(r.x + 14.0, r.y + 50.0, r.w - 28.0, r.h - 58.0);

    // (indent, text, size, colour), then the gap below it.
    let mut lines: Vec<(f32, String, f32, Color, f32)> = vec![];
    let para = |lines: &mut Vec<(f32, String, f32, Color, f32)>, indent: f32, s: &str, size: f32, c: Color, after: f32| {
        let wrapped = gfx::wrap(s, size, body.w - indent);
        let n = wrapped.len();
        for (i, l) in wrapped.into_iter().enumerate() {
            lines.push((indent, l, size, c, if i + 1 == n { after } else { 3.0 }));
        }
    };
    para(&mut lines, 0.0, HELP_INTRO, 13.0, pal::DIM, 14.0);
    for (cat, title) in HELP_CATS {
        let c = token_color(vm::TokKind::Op(cat));
        para(&mut lines, 0.0, title, 12.0, with_alpha(c, 0.8), 6.0);
        for m in vm::MNEMONICS.iter().filter(|m| m.cat == cat) {
            para(&mut lines, 8.0, m.syntax, 13.5, c, 2.0);
            para(&mut lines, 20.0, m.help, 13.0, pal::TEXT, 7.0);
        }
        if let Some(last) = lines.last_mut() {
            last.4 += 8.0;
        }
    }
    for (title, body_lines) in help_sections(ui.touch) {
        para(&mut lines, 0.0, title, 12.0, with_alpha(pal::VIOLET, 0.85), 6.0);
        for l in body_lines {
            para(&mut lines, 8.0, &l, 13.0, pal::TEXT, 5.0);
        }
        if let Some(last) = lines.last_mut() {
            last.4 += 8.0;
        }
    }

    let total: f32 = lines.iter().map(|l| l.2 + l.4).sum();
    if ui.hover(body) && ui.wheel != 0.0 {
        *scroll -= ui.wheel.signum() * 60.0;
    }
    if let Some(d) = ui.drag {
        *scroll -= d.y;
    }
    let max = (total - body.h).max(0.0);
    *scroll = scroll.clamp(0.0, max);
    let mut y = body.y - *scroll;
    for (indent, s, size, c, after) in &lines {
        if y >= body.y - 1.0 && y + size <= body.y + body.h {
            gfx::text(s, body.x + indent, y, *size, *c);
        }
        y += size + after;
    }
    if max > 0.0 {
        let bar = body.h * body.h / total;
        let by = body.y + (body.h - bar) * *scroll / max;
        draw_rectangle(r.x + r.w - 5.0, by, 2.0, bar, with_alpha(pal::ACCENT, 0.5));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The units are dronoids: the military word must never reach the screen.
    #[test]
    fn players_only_ever_read_dronoid() {
        let banned = concat!("dron", "e");
        let said = |text: &str, whose: &str| assert!(!text.to_lowercase().contains(banned), "{whose}: {text}");
        for q in QUESTS.iter() {
            said(q.title, "objective");
            said(q.hint, "objective");
            said(q.hint_touch, "objective");
        }
        for m in vm::MNEMONICS.iter() {
            said(m.syntax, "help");
            said(m.help, "help");
        }
        for b in programs::BUILTINS {
            said(b.source, b.name);
            said(b.summary, b.name);
        }
        said(programs::TEMPLATE, "template");
        for (page, text) in [
            ("game page", include_str!("../web/index.html").to_string()),
            // (Its former storage key is old data, not something shown.)
            ("god_net.js", include_str!("../web/god_net.js").replace(concat!("game-of-dron", "es."), "")),
            ("site", include_str!("../site/www/index.html").to_string()),
            ("site.js", include_str!("../site/www/site.js").to_string()),
            ("service", include_str!("../site/dronoids_site.py").to_string()),
        ] {
            said(&text, page);
        }
        // Every string literal of the code that draws the screens.
        let screens = [
            ("game.rs", include_str!("game.rs")),
            ("menu.rs", include_str!("menu.rs")),
            ("ui.rs", include_str!("ui.rs")),
            ("editor.rs", include_str!("editor.rs")),
            ("view.rs", include_str!("view.rs")),
            ("blocks.rs", include_str!("blocks.rs")),
        ];
        for (name, code) in screens {
            for line in code.lines().filter(|l| !l.trim_start().starts_with("//")) {
                for literal in line.split('"').skip(1).step_by(2) {
                    said(literal, name);
                }
            }
        }
    }
}
