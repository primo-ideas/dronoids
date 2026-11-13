//! The tasks panel: what the colony's dronoids are doing, and new tasks.
//! "These 10 go mining until 250 crystals are in", "these 3 explore
//! eastwards for 5 minutes": a kind, how many, until when, and a few options;
//! launched, stopped, launched again, repeated.

use macroquad::prelude::*;

use dronoids::rules::MAX_DRONOIDS_PER_PLAYER;
use dronoids::sim::{Command, Limit, Task, TaskKind, TaskSpec, TaskState, World};
use dronoids::vm;

use crate::gfx::{self, mix, pal, with_alpha};
use crate::library::Library;
use crate::ui::Ui;

/// What the panel asks of the game.
pub enum TaskOut {
    Issue(Command),
    /// Pick the raid's target on the map.
    PickTarget,
    /// Open the programs, at this one (custom tasks).
    EditPrograms(String),
}

/// A task being set up.
pub struct Form {
    pub spec: TaskSpec,
    /// The library program a custom task runs.
    program: usize,
}

#[derive(Default)]
pub struct TaskPanel {
    pub form: Option<Form>,
    scroll: f32,
}

const ROW: f32 = 44.0;
const HEAD: f32 = 34.0;

pub fn kind_color(k: TaskKind) -> Color {
    match k {
        TaskKind::Mine => pal::ORE,
        TaskKind::Explore => pal::ACCENT,
        TaskKind::Prospect => pal::VIOLET,
        TaskKind::Guard => pal::GOOD,
        TaskKind::Raid => pal::BAD,
        TaskKind::Custom => pal::PINK,
    }
}

fn state_text(t: &Task) -> (&'static str, Color) {
    match t.state {
        TaskState::Waiting => ("waiting for dronoids", pal::WARN),
        TaskState::Running => ("working", pal::GOOD),
        TaskState::Returning => ("coming home", pal::DIM),
        TaskState::Done => ("done", pal::FAINT),
    }
}

const HEADINGS: [(&str, i32); 5] = [("any", -1), ("N", 0), ("E", 90), ("S", 180), ("W", 270)];

impl TaskPanel {
    /// Open the form for a new task of this kind.
    pub fn start(&mut self, kind: TaskKind) {
        self.form = Some(Form { spec: TaskSpec::new(kind, 4), program: 0 });
    }

    /// Where the raid being set up heads for (world units).
    pub fn set_target(&mut self, x: i32, y: i32) {
        if let Some(f) = &mut self.form {
            f.spec.target = Some((x, y));
        }
    }

    pub fn height(&self, world: &World, me: u8) -> f32 {
        let tasks = world.tasks.iter().filter(|t| t.owner == me).count() as f32;
        let form = match &self.form {
            Some(f) => Self::form_height(f.spec.kind),
            None => 40.0,
        };
        HEAD + tasks * ROW + form + 6.0
    }

    fn form_height(kind: TaskKind) -> f32 {
        let options = match kind {
            TaskKind::Explore | TaskKind::Raid | TaskKind::Custom => 34.0,
            _ => 0.0,
        };
        8.0 + 2.0 * 30.0 + 22.0 + 3.0 * 34.0 + options + 40.0
    }

    pub fn ui(&mut self, ui: &mut Ui, world: &World, me: u8, lib: &Library, r: Rect, out: &mut Vec<TaskOut>) {
        ui.solid_panel(r);
        let (x, w) = (r.x + 10.0, r.w - 20.0);
        let idle = world.idle_count(me);
        gfx::text("Tasks", x, r.y + 9.0, 15.0, pal::ACCENT);
        let waiting = format!("{idle} waiting at home · {}/{} dronoids", world.dronoid_count(me), world.room(me));
        gfx::text(&waiting, x + w - gfx::text_width(&waiting, 12.0), r.y + 11.0, 12.0, if idle > 0 { pal::TEXT } else { pal::DIM });

        // Scrolling, when there are more tasks than room.
        let body = Rect::new(r.x, r.y + HEAD, r.w, r.h - HEAD);
        if ui.hover(body) && ui.wheel != 0.0 {
            self.scroll -= ui.wheel.signum() * 40.0;
        }
        if ui.touch && ui.hover(body) && let Some(d) = ui.drag {
            self.scroll -= d.y;
        }
        let content = self.height(world, me) - HEAD;
        self.scroll = self.scroll.clamp(0.0, (content - body.h).max(0.0));
        let mut y = body.y - self.scroll;
        let shown = |y: f32, h: f32| y >= body.y - 1.0 && y + h <= body.y + body.h + 1.0;

        for t in world.tasks.iter().filter(|t| t.owner == me) {
            if shown(y, ROW) {
                self.task_row(ui, world, t, Rect::new(x, y, w, ROW - 6.0), out);
            }
            y += ROW;
        }
        if world.tasks.iter().all(|t| t.owner != me) && shown(y, 20.0) {
            gfx::text("No task: your dronoids wait at home.", x, y + 2.0, 12.0, pal::DIM);
            y += 20.0;
        }

        match self.form.take() {
            None => {
                if shown(y, 34.0) && ui.button_primary(Rect::new(x, y + 4.0, w, 30.0), "New task", pal::ACCENT) {
                    self.start(TaskKind::Mine);
                }
            }
            Some(mut f) => {
                let keep = self.form_ui(ui, world, me, lib, &mut f, x, y, w, out);
                if keep {
                    self.form = Some(f);
                }
            }
        }
    }

    fn task_row(&self, ui: &mut Ui, world: &World, t: &Task, r: Rect, out: &mut Vec<TaskOut>) {
        let col = kind_color(t.spec.kind);
        draw_rectangle(r.x, r.y, 3.0, r.h, with_alpha(col, 0.8));
        draw_rectangle(r.x, r.y, r.w, r.h, with_alpha(col, 0.07));
        let title = t.spec.title();
        gfx::text(&title, r.x + 10.0, r.y + 4.0, gfx::fit_size(&title, 14.0, r.w * 0.55), mix(col, WHITE, 0.3));
        let (state, sc) = state_text(t);
        let n = world.task_dronoids(t.id);
        let info = match t.state {
            TaskState::Running | TaskState::Waiting => format!("{state} · {n}/{}", t.spec.count),
            TaskState::Returning => format!("{state} · {n} out"),
            TaskState::Done if t.runs > 1 => format!("done {} times", t.runs),
            TaskState::Done => state.to_string(),
        };
        gfx::text(&info, r.x + 10.0, r.y + 22.0, 11.0, sc);
        // How far along it is.
        let (done, goal) = t.done_and_goal(world.tick);
        let limit = t.spec.limit.label();
        let bar = Rect::new(r.x + r.w * 0.40, r.y + 26.0, r.w * 0.22, 4.0);
        if goal > 0 && t.state == TaskState::Running {
            draw_rectangle(bar.x, bar.y, bar.w, bar.h, with_alpha(pal::FAINT, 0.7));
            draw_rectangle(bar.x, bar.y, bar.w * (done as f32 / goal as f32).clamp(0.0, 1.0), bar.h, col);
        }
        if ui.hover(r) {
            ui.tooltip(&format!("{} — {} · until {limit}{}", t.spec.kind.name(), t.spec.kind.summary(), if t.spec.repeat { " · repeats" } else { "" }));
        }
        // From the right: delete, repeat, stop or again.
        let bh = 24.0;
        let del = Rect::new(r.x + r.w - 26.0, r.y + 7.0, 26.0, bh);
        let rep = Rect::new(del.x - 34.0, del.y, 30.0, bh);
        let act = Rect::new(rep.x - 58.0, del.y, 54.0, bh);
        if ui.button(del, "×") {
            out.push(TaskOut::Issue(Command::DeleteTask { id: t.id }));
        }
        if ui.hover(del) {
            ui.tooltip("Delete the task: its dronoids are free at once");
        }
        if ui.button_ex(rep, "R", t.spec.repeat, true, pal::ACCENT) {
            out.push(TaskOut::Issue(Command::EditTask { id: t.id, count: t.spec.count, limit: t.spec.limit, repeat: !t.spec.repeat }));
        }
        if ui.hover(rep) {
            ui.tooltip(if t.spec.repeat { "Repeats once done (click: stop repeating)" } else { "Repeat it once done" });
        }
        match t.state {
            TaskState::Running | TaskState::Waiting => {
                if ui.button_ex(act, "Stop", false, true, pal::WARN) {
                    out.push(TaskOut::Issue(Command::StopTask { id: t.id }));
                }
            }
            TaskState::Done => {
                if ui.button_ex(act, "Again", false, true, pal::GOOD) {
                    out.push(TaskOut::Issue(Command::RelaunchTask { id: t.id }));
                }
            }
            TaskState::Returning => {}
        }
    }

    /// The form for a new task. Returns whether it stays open.
    #[allow(clippy::too_many_arguments)]
    fn form_ui(&self, ui: &mut Ui, world: &World, me: u8, lib: &Library, f: &mut Form, x: f32, y: f32, w: f32, out: &mut Vec<TaskOut>) -> bool {
        let mut y = y + 8.0;
        draw_line(x, y - 4.0, x + w, y - 4.0, 1.0, with_alpha(pal::FAINT, 0.8));
        // The kind, two rows of three.
        let cw = (w - 8.0) / 3.0;
        for (i, k) in TaskKind::ALL.into_iter().enumerate() {
            let br = Rect::new(x + (i % 3) as f32 * (cw + 4.0), y + (i / 3) as f32 * 30.0, cw, 26.0);
            if ui.button_ex(br, k.name(), f.spec.kind == k, true, kind_color(k)) && f.spec.kind != k {
                let count = f.spec.count;
                f.spec = TaskSpec::new(k, count);
            }
        }
        y += 60.0;
        gfx::text(f.spec.kind.summary(), x, y + 2.0, gfx::fit_size(f.spec.kind.summary(), 12.0, w), pal::DIM);
        y += 22.0;

        let label = |s: &str, y: f32| gfx::text(s, x, y + 7.0, 12.0, pal::DIM);
        // How many.
        label("dronoids", y);
        let count = format!("{}", f.spec.count);
        let cx = x + 90.0;
        if ui.button(Rect::new(cx, y, 28.0, 26.0), "−") && f.spec.count > 1 {
            f.spec.count -= 1;
        }
        gfx::text_centered(&count, cx + 52.0, y + 4.0, 16.0, pal::ACCENT);
        if ui.button(Rect::new(cx + 76.0, y, 28.0, 26.0), "+") && (f.spec.count as usize) < MAX_DRONOIDS_PER_PLAYER {
            f.spec.count += 1;
        }
        if ui.button(Rect::new(cx + 108.0, y, 36.0, 26.0), "+5") {
            f.spec.count = (f.spec.count as usize + 5).min(MAX_DRONOIDS_PER_PLAYER) as u8;
        }
        let idle = format!("{} free", world.idle_count(me));
        gfx::text(&idle, x + w - gfx::text_width(&idle, 11.0), y + 8.0, 11.0, pal::DIM);
        y += 34.0;
        // Until when.
        label("until", y);
        let limits = f.spec.kind.limits();
        let at = limits.iter().position(|l| *l == f.spec.limit).unwrap_or(0);
        if ui.button_ex(Rect::new(cx, y, w - 90.0, 26.0), &f.spec.limit.label(), false, true, pal::ACCENT) {
            f.spec.limit = limits[(at + 1) % limits.len()];
        }
        y += 34.0;
        // Options of the kind.
        match f.spec.kind {
            TaskKind::Explore => {
                label("heading", y);
                let hw = (w - 90.0 - 4.0 * 4.0) / 5.0;
                for (i, (name, dir)) in HEADINGS.iter().enumerate() {
                    let br = Rect::new(cx + i as f32 * (hw + 4.0), y, hw, 26.0);
                    if ui.button_ex(br, name, f.spec.dir == *dir, true, pal::ACCENT) {
                        f.spec.dir = *dir;
                    }
                }
                y += 34.0;
            }
            TaskKind::Raid => {
                label("target", y);
                let text = f.spec.target.map_or("none: they patrol".to_string(), |(tx, ty)| format!("{tx}, {ty}"));
                if ui.button_ex(Rect::new(cx, y, w - 90.0 - 30.0, 26.0), &text, false, true, pal::PINK) {
                    out.push(TaskOut::PickTarget);
                }
                if f.spec.target.is_some() && ui.button(Rect::new(x + w - 26.0, y, 26.0, 26.0), "×") {
                    f.spec.target = None;
                }
                y += 34.0;
            }
            TaskKind::Custom => {
                label("program", y);
                let names: Vec<&str> = lib.entries.iter().map(|e| e.name.as_str()).collect();
                f.program = f.program.min(names.len().saturating_sub(1));
                let name = names.get(f.program).copied().unwrap_or("?");
                if ui.button_ex(Rect::new(cx, y, w - 90.0 - 58.0, 26.0), name, false, true, pal::PINK) && !names.is_empty() {
                    f.program = (f.program + 1) % names.len();
                }
                if ui.button(Rect::new(x + w - 54.0, y, 54.0, 26.0), "Edit") {
                    out.push(TaskOut::EditPrograms(name.to_string()));
                }
                y += 34.0;
            }
            _ => {}
        }
        // Repeat.
        label("once done", y);
        let rep = if f.spec.repeat { "start over" } else { "stop" };
        if ui.button_ex(Rect::new(cx, y, w - 90.0, 26.0), rep, f.spec.repeat, true, pal::ACCENT) {
            f.spec.repeat = !f.spec.repeat;
        }
        y += 36.0;
        // Launch, or not.
        let half = (w - 6.0) / 2.0;
        let mut keep = true;
        if ui.button_primary(Rect::new(x, y, half, 30.0), "Launch", pal::GOOD) {
            let mut spec = f.spec.clone();
            if spec.kind == TaskKind::Custom {
                let Some(e) = lib.entries.get(f.program) else { return true };
                if vm::assemble(&e.name, &e.source).is_err() {
                    out.push(TaskOut::EditPrograms(e.name.clone()));
                    return true;
                }
                spec.program = Some((e.name.clone(), e.source.clone()));
            }
            if spec.limit == Limit::Never && spec.repeat {
                spec.repeat = false;
            }
            out.push(TaskOut::Issue(Command::NewTask { spec }));
            keep = false;
        }
        if ui.button(Rect::new(x + half + 6.0, y, half, 30.0), "Cancel") {
            keep = false;
        }
        keep
    }
}
