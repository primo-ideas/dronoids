//! The tile editor: a program as a column of instruction tiles, in the
//! spirit of Human Resource Machine. Tiles are dragged in from a palette,
//! dragged around to reorder them and out of the program to delete them; a
//! tile's operands are tokens, and touching one offers what it can be. Each
//! jump is drawn as an arrow to its label.
//!
//! The program stays DASM text: the tiles are read from it every frame and
//! written back to it on every change, so the text editor, the assembler
//! and the saves never see the difference.

use macroquad::prelude::*;

use dronoids::vm::{self, Arg, AsmError, TokKind};

use crate::editor::token_color;
use crate::gfx::{self, mix, pal, with_alpha};
use crate::ui::Ui;

/// One line of a program, as a tile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    Label(String),
    Op { op: String, args: Vec<String>, note: String },
    /// A comment on a line of its own.
    Note(String),
    /// An empty line: a little room between tiles.
    Gap,
}

/// The tiles of a source, each with the source line it comes from.
pub fn parse(source: &str) -> Vec<(Item, usize)> {
    let mut out = vec![];
    for (ln, line) in source.lines().enumerate() {
        let code_end = vm::strip_comment(line).len();
        let comment = line[code_end..].trim_start_matches([';', '#', '/']).trim().to_string();
        let mut rest = line[..code_end].trim();
        while let Some(i) = rest.find(':') {
            let name = rest[..i].trim();
            if name.is_empty() || name.contains(char::is_whitespace) {
                break;
            }
            out.push((Item::Label(name.to_string()), ln));
            rest = rest[i + 1..].trim();
        }
        if !rest.is_empty() {
            let mut toks = rest.split(|c: char| c.is_whitespace() || c == ',').filter(|t| !t.is_empty());
            let op = toks.next().unwrap_or_default().to_string();
            let args = toks.map(str::to_string).collect();
            out.push((Item::Op { op, args, note: comment }, ln));
        } else if !comment.is_empty() {
            out.push((Item::Note(comment), ln));
        } else if line.trim().is_empty() {
            out.push((Item::Gap, ln));
        }
    }
    out
}

/// The source of a column of tiles.
pub fn render(items: &[Item]) -> String {
    let mut out = String::new();
    for it in items {
        match it {
            Item::Label(name) => out += &format!("{name}:\n"),
            Item::Op { op, args, note } => {
                let mut line = format!("    {op}");
                if !args.is_empty() {
                    line += " ";
                    line += &args.join(", ");
                }
                if !note.is_empty() {
                    let pad = 25usize.saturating_sub(line.chars().count()).max(1);
                    line += &" ".repeat(pad);
                    line += "; ";
                    line += note;
                }
                out += &line;
                out.push('\n');
            }
            Item::Note(text) => out += &format!("; {text}\n"),
            Item::Gap => out.push('\n'),
        }
    }
    out
}

/// What can be put in an operand of that kind.
fn choices(kind: Arg, labels: &[String]) -> Vec<String> {
    let regs = ["a", "b", "c", "d"];
    let list: Vec<&str> = match kind {
        Arg::T => vec!["ORE", "BASE", "ENEMY", "EBASE", "RELIC", "ALLY", "WALL", "EMPTY", "a", "b", "c", "d"],
        Arg::A => vec!["N", "E", "S", "W", "a", "b", "c", "d"],
        Arg::R => regs.to_vec(),
        Arg::V => vec!["a", "b", "c", "d", "ok", "0", "1", "2", "3", "5", "10", "90", "180", "270", "360", "FULL", "NONE"],
        Arg::L => return labels.to_vec(),
    };
    list.into_iter().map(str::to_string).collect()
}

/// What a new tile's operands start as.
fn default_arg(kind: Arg, labels: &[String]) -> String {
    match kind {
        Arg::T => "ORE".into(),
        Arg::A => "N".into(),
        Arg::R => "a".into(),
        Arg::V => "0".into(),
        Arg::L => labels.first().cloned().unwrap_or_else(|| "start".into()),
    }
}

fn labels_of(items: &[Item]) -> Vec<String> {
    items.iter().filter_map(|i| if let Item::Label(n) = i { Some(n.clone()) } else { None }).collect()
}

/// A label name not used yet.
fn fresh_label(items: &[Item]) -> String {
    let used = labels_of(items);
    (1..).map(|n| format!("spot{n}")).find(|n| !used.iter().any(|u| u.eq_ignore_ascii_case(n))).unwrap()
}

/// What a tile is made from, in the palette.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Op(&'static str),
    Label,
}

impl Kind {
    /// A new item of this kind, for a program that has these items.
    fn make(self, items: &[Item]) -> Item {
        match self {
            Kind::Label => Item::Label(fresh_label(items)),
            Kind::Op(name) => {
                let labels = labels_of(items);
                let m = vm::mnemonic(name).expect("a mnemonic");
                Item::Op { op: m.name.to_string(), args: m.args.iter().map(|a| default_arg(*a, &labels)).collect(), note: String::new() }
            }
        }
    }
}

#[derive(Clone, Debug)]
enum Source {
    Palette(Kind),
    Program(usize),
}

#[derive(Clone, Debug)]
struct Press {
    from: Source,
    at: Vec2,
    dragging: bool,
}

#[derive(Default)]
pub struct Blocks {
    scroll: f32,
    palette_scroll: f32,
    press: Option<Press>,
    /// The operand whose choices are open: tile, operand.
    picker: Option<(usize, usize)>,
    /// The palette also shows the advanced instructions.
    more: bool,
    /// Sources before each change, for Undo.
    history: Vec<String>,
}

const OP_H: f32 = 28.0;
const LABEL_H: f32 = 22.0;
const NOTE_H: f32 = 18.0;
const GAP_H: f32 = 6.0;
const SPACING: f32 = 4.0;
/// Room left of the tiles for the jump arrows.
const MARGIN: f32 = 30.0;
const DRAG_SLOP: f32 = 6.0;

fn height(it: &Item) -> f32 {
    match it {
        Item::Op { .. } => OP_H,
        Item::Label(_) => LABEL_H,
        Item::Note(_) => NOTE_H,
        Item::Gap => GAP_H,
    }
}

fn op_color(op: &str) -> Color {
    match vm::mnemonic(op) {
        Some(m) => token_color(TokKind::Op(m.cat)),
        None => pal::BAD,
    }
}

fn arg_color(tok: &str) -> Color {
    let kind = if vm::parse_reg(tok).is_some() {
        TokKind::Reg
    } else if tok.parse::<i32>().is_ok() {
        TokKind::Number
    } else if vm::constant(tok).is_some() {
        TokKind::Const
    } else {
        TokKind::Label
    };
    token_color(kind)
}

/// How an operand reads on its token.
fn arg_text(kind: Option<Arg>, tok: &str) -> String {
    if kind == Some(Arg::L) { format!("> {tok}") } else { tok.to_string() }
}

/// `text` cut to fit `w` pixels at `size`, ending with an ellipsis.
fn clip(text: &str, size: f32, w: f32) -> String {
    if gfx::text_width(text, size) <= w {
        return text.to_string();
    }
    let mut out: String = text.to_string();
    while !out.is_empty() && gfx::text_width(&format!("{out}…"), size) > w {
        out.pop();
    }
    if out.is_empty() { String::new() } else { format!("{}…", out.trim_end()) }
}

/// Where the program's tiles go, and the palette.
struct Layout {
    program: Rect,
    tiles_x: f32,
    palette: Option<Rect>,
}

impl Blocks {
    fn layout(r: Rect, read_only: bool) -> Layout {
        let palette_w = if read_only { 0.0 } else if r.w > 560.0 { 176.0 } else { (r.w * 0.36).clamp(104.0, 150.0) };
        let program = Rect::new(r.x, r.y, r.w - palette_w - if read_only { 0.0 } else { 8.0 }, r.h);
        let palette = (!read_only).then(|| Rect::new(program.x + program.w + 8.0, r.y, palette_w, r.h));
        Layout { program, tiles_x: program.x + MARGIN, palette }
    }

    /// The tiles' tops in the program column (before scrolling).
    fn tops(items: &[Item]) -> (Vec<f32>, f32) {
        let mut y = 6.0;
        let mut tops = Vec::with_capacity(items.len());
        for it in items {
            tops.push(y);
            y += height(it) + SPACING;
        }
        (tops, y)
    }

    /// Where a tile dropped at height `my` (screen) goes.
    fn insert_at(&self, items: &[Item], tops: &[f32], top: f32, my: f32) -> usize {
        for (i, it) in items.iter().enumerate() {
            if my < top + tops[i] - self.scroll + height(it) / 2.0 {
                return i;
            }
        }
        items.len()
    }

    /// Tiles of the palette, in order.
    fn palette_kinds(&self) -> Vec<Kind> {
        let mut out: Vec<Kind> = vm::BASIC.iter().copied().map(Kind::Op).collect();
        out.insert(0, Kind::Label);
        if self.more {
            out.extend(vm::MNEMONICS.iter().filter(|m| !m.basic()).map(|m| Kind::Op(m.name)));
        }
        out
    }

    /// Draw the program as tiles and handle what is done to them. Returns
    /// the new source when it changed. `exec_line` marks the tile being run
    /// (the debugger), which `read_only` programs scroll to.
    pub fn ui(&mut self, ui: &mut Ui, r: Rect, source: &str, errors: &[AsmError], exec_line: Option<usize>, read_only: bool) -> Option<String> {
        let parsed = parse(source);
        let items: Vec<Item> = parsed.iter().map(|p| p.0.clone()).collect();
        let open_before = self.picker;
        let lines: Vec<usize> = parsed.iter().map(|p| p.1).collect();
        let l = Blocks::layout(r, read_only);
        let (tops, total) = Blocks::tops(&items);
        let pr = l.program;
        let max_scroll = (total - pr.h + 40.0).max(0.0);

        // Scrolling: the wheel, or a finger sliding on the program.
        if ui.hover(pr) && ui.wheel != 0.0 {
            self.scroll -= ui.wheel.signum() * 60.0;
        }
        // (A finger on a tile drags the tile; on the empty part, it scrolls.)
        if ui.touch
            && let Some(d) = ui.drag
            && self.press.is_none()
            && ui.hover(pr)
        {
            self.scroll -= d.y;
        }
        if read_only && let Some(line) = exec_line
            && let Some(i) = lines.iter().position(|l| *l == line)
        {
            let (t, b) = (tops[i] - self.scroll, tops[i] + height(&items[i]) - self.scroll);
            if t < 0.0 || b > pr.h {
                self.scroll = tops[i] - pr.h / 3.0;
            }
        }
        self.scroll = self.scroll.clamp(0.0, max_scroll);

        let mut changed: Option<Vec<Item>> = None;
        let labels = labels_of(&items);

        // ----- the program -----
        draw_rectangle(pr.x, pr.y, pr.w, pr.h, with_alpha(BLACK, 0.25));
        let tile_w = pr.x + pr.w - 6.0 - l.tiles_x;
        let tile_rect = |i: usize, scroll: f32| Rect::new(l.tiles_x, pr.y + tops[i] - scroll, tile_w, height(&items[i]));
        let visible = |rr: Rect| rr.y >= pr.y && rr.y + rr.h <= pr.y + pr.h;
        let dragged = match &self.press {
            Some(Press { from: Source::Program(i), dragging: true, .. }) => Some(*i),
            _ => None,
        };

        // Jump arrows, in lanes, from the jump to its label.
        let mut lane = 0;
        for (i, it) in items.iter().enumerate() {
            let Item::Op { op, args, .. } = it else { continue };
            let Some(m) = vm::mnemonic(op) else { continue };
            let Some(k) = m.args.iter().position(|a| *a == Arg::L) else { continue };
            let Some(target) = args.get(k) else { continue };
            let Some(t) = items.iter().position(|x| matches!(x, Item::Label(n) if n.eq_ignore_ascii_case(target))) else { continue };
            let (a, b) = (tile_rect(i, self.scroll), tile_rect(t, self.scroll));
            let (ya, yb) = (a.y + a.h / 2.0, b.y + b.h / 2.0);
            if (ya < pr.y && yb < pr.y) || (ya > pr.y + pr.h && yb > pr.y + pr.h) {
                lane += 1;
                continue;
            }
            let x = pr.x + 5.0 + (lane % 4) as f32 * 6.0;
            lane += 1;
            let col = with_alpha(token_color(TokKind::Label), if Some(i) == dragged { 0.15 } else { 0.55 });
            let clip = |y: f32| y.clamp(pr.y, pr.y + pr.h);
            draw_line(a.x, clip(ya), x, clip(ya), 1.5, col);
            draw_line(x, clip(ya), x, clip(yb), 1.5, col);
            if yb >= pr.y && yb <= pr.y + pr.h {
                draw_line(x, yb, b.x - 2.0, yb, 1.5, col);
                draw_triangle(vec2(b.x - 1.0, yb), vec2(b.x - 7.0, yb - 4.0), vec2(b.x - 7.0, yb + 4.0), col);
            }
        }

        let mut chip_hit: Option<(usize, usize)> = None;
        let mut tile_hit: Option<usize> = None;
        for (i, it) in items.iter().enumerate() {
            let rr = tile_rect(i, self.scroll);
            if !visible(rr) {
                continue;
            }
            let ghost = Some(i) == dragged;
            let error = errors.iter().any(|e| e.line == lines[i]);
            let running = exec_line == Some(lines[i]) && matches!(it, Item::Op { .. });
            let chips = self.draw_item(it, rr, ghost, error, running);
            if ui.hover(rr) {
                tile_hit = Some(i);
                for (k, c) in chips.iter().enumerate() {
                    if c.contains(ui.mouse) {
                        chip_hit = Some((i, k));
                    }
                }
                if let Item::Op { op, .. } = it
                    && let Some(m) = vm::mnemonic(op)
                    && self.press.is_none()
                    && self.picker.is_none()
                {
                    let msg = errors.iter().find(|e| e.line == lines[i]).map_or(String::new(), |e| format!(" · {}", e.msg));
                    ui.tooltip(&format!("{} — {}{msg}", m.syntax, m.help));
                } else if error && let Some(e) = errors.iter().find(|e| e.line == lines[i]) {
                    ui.tooltip(&e.msg);
                }
            }
        }
        if items.is_empty() && !read_only {
            let hint = "Drag instructions here from the right";
            gfx::text_centered(hint, pr.x + pr.w / 2.0, pr.y + 30.0, gfx::fit_size(hint, 13.0, pr.w - 20.0), pal::DIM);
        }
        ui.block(pr);

        // ----- the palette -----
        let kinds = self.palette_kinds();
        let mut palette_hit: Option<Kind> = None;
        if let Some(pal_r) = l.palette {
            draw_rectangle(pal_r.x, pal_r.y, pal_r.w, pal_r.h, with_alpha(pal::PANEL, 0.6));
            let row = 30.0;
            let head = 30.0;
            let undo = Rect::new(pal_r.x + 4.0, pal_r.y + 3.0, (pal_r.w - 12.0) / 2.0, 24.0);
            if ui.button_ex(undo, "Undo", false, !self.history.is_empty(), pal::ACCENT)
                && let Some(prev) = self.history.pop()
            {
                return Some(prev);
            }
            let more = Rect::new(undo.x + undo.w + 4.0, undo.y, undo.w, 24.0);
            if ui.button_ex(more, if self.more { "Fewer" } else { "More" }, self.more, true, pal::ACCENT) {
                self.more = !self.more;
            }
            let body = Rect::new(pal_r.x, pal_r.y + head, pal_r.w, pal_r.h - head);
            let content = kinds.len() as f32 * row + 8.0;
            if ui.hover(body) && ui.wheel != 0.0 {
                self.palette_scroll -= ui.wheel.signum() * 60.0;
            }
            if ui.touch && ui.hover(body) && self.press.is_none() && let Some(d) = ui.drag {
                self.palette_scroll -= d.y;
            }
            self.palette_scroll = self.palette_scroll.clamp(0.0, (content - body.h).max(0.0));
            for (k, kind) in kinds.iter().enumerate() {
                let tr = Rect::new(body.x + 6.0, body.y + 4.0 + k as f32 * row - self.palette_scroll, body.w - 12.0, row - 6.0);
                if tr.y < body.y || tr.y + tr.h > body.y + body.h {
                    continue;
                }
                let (name, col) = match kind {
                    Kind::Label => ("label".to_string(), token_color(TokKind::LabelDef)),
                    Kind::Op(n) => (n.to_ascii_lowercase(), op_color(n)),
                };
                let hot = ui.hover(tr);
                draw_rectangle(tr.x, tr.y, tr.w, tr.h, with_alpha(col, if hot { 0.28 } else { 0.14 }));
                draw_rectangle_lines(tr.x, tr.y, tr.w, tr.h, 1.0, with_alpha(col, 0.7));
                gfx::text(&name, tr.x + 8.0, tr.y + 5.0, gfx::fit_size(&name, 13.0, tr.w - 12.0), mix(col, WHITE, 0.3));
                if hot {
                    palette_hit = Some(*kind);
                    if self.press.is_none() {
                        ui.tooltip(&match kind {
                            Kind::Label => "label — a place in the program that jumps go to".to_string(),
                            Kind::Op(n) => vm::mnemonic(n).map_or(String::new(), |m| format!("{} — {}", m.syntax, m.help)),
                        });
                    }
                }
            }
            ui.block(pal_r);
        }

        // ----- the operand picker -----
        if let Some((i, k)) = self.picker {
            let kind = match items.get(i) {
                Some(Item::Op { op, args, .. }) if k < args.len() => vm::mnemonic(op).and_then(|m| m.args.get(k).copied()),
                _ => None,
            };
            match kind {
                Some(kind) if !read_only => {
                    let opts = choices(kind, &labels);
                    let rr = tile_rect(i, self.scroll);
                    let cols = 4usize;
                    let (cw, ch) = (((pr.w - 20.0) / cols as f32).min(80.0), 26.0);
                    let rows = opts.len().div_ceil(cols).max(1);
                    let ph = rows as f32 * (ch + 4.0) + 8.0;
                    let py = if rr.y + rr.h + ph < pr.y + pr.h { rr.y + rr.h + 2.0 } else { (rr.y - ph - 2.0).max(pr.y) };
                    let panel = Rect::new(l.tiles_x, py, cols as f32 * (cw + 4.0) + 4.0, ph);
                    ui.solid_panel(panel);
                    if opts.is_empty() {
                        gfx::text("add a label first", panel.x + 8.0, panel.y + 10.0, 12.0, pal::DIM);
                    }
                    for (n, o) in opts.iter().enumerate() {
                        let br = Rect::new(panel.x + 4.0 + (n % cols) as f32 * (cw + 4.0), panel.y + 4.0 + (n / cols) as f32 * (ch + 4.0), cw, ch);
                        let current = matches!(&items[i], Item::Op { args, .. } if args[k].eq_ignore_ascii_case(o));
                        if ui.button_ex(br, o, current, true, arg_color(o)) {
                            let mut next = items.clone();
                            if let Item::Op { args, .. } = &mut next[i] {
                                args[k] = o.clone();
                            }
                            changed = Some(next);
                            self.picker = None;
                        }
                    }
                    // A touch anywhere else closes it (another token opens its own).
                    if ui.pressed && !panel.contains(ui.mouse) {
                        self.picker = None;
                    }
                }
                _ => self.picker = None,
            }
        }

        // ----- pressing, dragging, dropping -----
        if !read_only {
            if ui.pressed {
                if let Some((i, k)) = chip_hit {
                    self.picker = if open_before == Some((i, k)) { None } else { Some((i, k)) };
                    ui.pressed = false;
                } else if let Some(i) = tile_hit {
                    self.press = Some(Press { from: Source::Program(i), at: ui.mouse, dragging: false });
                    self.picker = None;
                    ui.pressed = false;
                } else if let Some(kind) = palette_hit {
                    self.press = Some(Press { from: Source::Palette(kind), at: ui.mouse, dragging: false });
                    self.picker = None;
                    ui.pressed = false;
                }
            }
            if let Some(p) = &mut self.press
                && !p.dragging
                && ui.down
                && ui.mouse.distance(p.at) > DRAG_SLOP
            {
                p.dragging = true;
            }
            // Left of the program counts too: only the palette's side deletes.
            let over_program = ui.mouse.x < pr.x + pr.w && ui.mouse.y >= pr.y && ui.mouse.y <= pr.y + pr.h;
            if let Some(p) = self.press.clone() {
                let item = match &p.from {
                    Source::Palette(kind) => kind.make(&items),
                    Source::Program(i) => items[*i].clone(),
                };
                if p.dragging && ui.down {
                    // The tile follows the pointer; a line shows where it would go.
                    if over_program {
                        let at = self.insert_at(&items, &tops, pr.y, ui.mouse.y);
                        let y = if at < items.len() { pr.y + tops[at] - self.scroll - SPACING / 2.0 } else { pr.y + total - self.scroll - SPACING };
                        draw_line(l.tiles_x - 6.0, y, l.tiles_x + tile_w, y, 2.0, pal::ACCENT);
                        // Near the edges, the program scrolls along.
                        if ui.mouse.y < pr.y + 24.0 {
                            self.scroll = (self.scroll - 6.0).max(0.0);
                        } else if ui.mouse.y > pr.y + pr.h - 24.0 {
                            self.scroll = (self.scroll + 6.0).min(max_scroll);
                        }
                    } else if matches!(p.from, Source::Program(_)) {
                        let bin = "drop here to delete";
                        if let Some(pal_r) = l.palette {
                            draw_rectangle(pal_r.x, pal_r.y, pal_r.w, pal_r.h, with_alpha(pal::BAD, 0.12));
                            gfx::text_centered(bin, pal_r.x + pal_r.w / 2.0, pal_r.y + pal_r.h - 24.0, gfx::fit_size(bin, 12.0, pal_r.w - 8.0), pal::BAD);
                        }
                    }
                    let fr = Rect::new(ui.mouse.x - 30.0, ui.mouse.y - height(&item) / 2.0, tile_w.min(220.0), height(&item));
                    self.draw_item(&item, fr, false, false, false);
                } else if !ui.down {
                    self.press = None;
                    let mut next = items.clone();
                    let mut edited = false;
                    if p.dragging {
                        match (&p.from, over_program) {
                            (Source::Program(i), true) => {
                                let mut at = self.insert_at(&items, &tops, pr.y, ui.mouse.y);
                                let moved = next.remove(*i);
                                if at > *i {
                                    at -= 1;
                                }
                                next.insert(at.min(next.len()), moved);
                                edited = at != *i;
                            }
                            (Source::Program(i), false) => {
                                next.remove(*i);
                                edited = true;
                            }
                            (Source::Palette(_), true) => {
                                let at = self.insert_at(&items, &tops, pr.y, ui.mouse.y);
                                next.insert(at, item);
                                edited = true;
                            }
                            (Source::Palette(_), false) => {}
                        }
                    } else if let Source::Palette(_) = p.from {
                        // A tap on the palette adds the tile at the end.
                        next.push(item);
                        edited = true;
                    }
                    if edited {
                        // A jump needs somewhere to go: a first label at the top.
                        let needs = next.iter().any(|it| matches!(it, Item::Op { op, args, .. } if vm::mnemonic(op).is_some_and(|m| m.args.iter().zip(args).any(|(a, v)| *a == Arg::L && v == "start"))));
                        if needs && !labels_of(&next).iter().any(|n| n.eq_ignore_ascii_case("start")) {
                            next.insert(0, Item::Label("start".into()));
                        }
                        changed = Some(next);
                    }
                }
            }
            // Ctrl+Z undoes the last change made with the tiles.
            if ui.ctrl && is_key_pressed(KeyCode::Z) && let Some(prev) = self.history.pop() {
                return Some(prev);
            }
        }
        let next = changed?;
        self.history.push(source.to_string());
        if self.history.len() > 100 {
            self.history.remove(0);
        }
        Some(render(&next))
    }

    /// Draw one tile; returns where its operand tokens are.
    fn draw_item(&self, it: &Item, r: Rect, ghost: bool, error: bool, running: bool) -> Vec<Rect> {
        let fade = if ghost { 0.3 } else { 1.0 };
        let mut chips = vec![];
        match it {
            Item::Op { op, args, note } => {
                let col = op_color(op);
                draw_rectangle(r.x, r.y, r.w, r.h, with_alpha(col, (if running { 0.4 } else { 0.15 }) * fade));
                let edge = if error { pal::BAD } else { with_alpha(col, 0.75 * fade) };
                draw_rectangle_lines(r.x, r.y, r.w, r.h, if error || running { 2.0 } else { 1.0 }, edge);
                if running {
                    draw_rectangle(r.x - 5.0, r.y, 3.0, r.h, pal::WARN);
                }
                let name = op.to_ascii_lowercase();
                let kinds = vm::mnemonic(op).map(|m| m.args).unwrap_or(&[]);
                let texts: Vec<String> = args.iter().enumerate().map(|(k, a)| arg_text(kinds.get(k).copied(), a)).collect();
                // Smaller letters for a tile that would not fit otherwise.
                let need = |s: f32| gfx::text_width(&name, s + 1.0) + 16.0 + texts.iter().map(|t| gfx::text_width(t, s) + 17.0).sum::<f32>();
                let size = [13.0, 12.0, 11.0, 10.0].into_iter().find(|s| need(*s) <= r.w).unwrap_or(10.0);
                let mut x = r.x + 8.0;
                gfx::text(&name, x, r.y + 7.0, size + 1.0, with_alpha(mix(col, WHITE, 0.35), fade));
                x += gfx::text_width(&name, size + 1.0) + 8.0;
                for (a, text) in args.iter().zip(&texts) {
                    let w = gfx::text_width(text, size) + 12.0;
                    let cr = Rect::new(x, r.y + 4.0, w, r.h - 8.0);
                    let ac = arg_color(a);
                    draw_rectangle(cr.x, cr.y, cr.w, cr.h, with_alpha(ac, 0.18 * fade));
                    draw_rectangle_lines(cr.x, cr.y, cr.w, cr.h, 1.0, with_alpha(ac, 0.6 * fade));
                    gfx::text(text, cr.x + 6.0, cr.y + 3.0 + (13.0 - size) / 2.0, size, with_alpha(ac, fade));
                    chips.push(cr);
                    x += w + 5.0;
                }
                if !note.is_empty() && x < r.x + r.w - 30.0 {
                    let shown = clip(note, 11.0, r.x + r.w - x - 10.0);
                    gfx::text(&shown, x + 4.0, r.y + 8.0, 11.0, with_alpha(token_color(TokKind::Comment), fade));
                }
            }
            Item::Label(name) => {
                let col = token_color(TokKind::LabelDef);
                let w = (gfx::text_width(name, 13.0) + 22.0).min(r.w);
                draw_rectangle(r.x, r.y, w, r.h, with_alpha(col, 0.14 * fade));
                draw_triangle(vec2(r.x + w, r.y), vec2(r.x + w + 8.0, r.y + r.h / 2.0), vec2(r.x + w, r.y + r.h), with_alpha(col, 0.14 * fade));
                draw_rectangle(r.x, r.y, 3.0, r.h, with_alpha(col, 0.8 * fade));
                let edge = if error { pal::BAD } else { with_alpha(col, 0.0) };
                if error {
                    draw_rectangle_lines(r.x, r.y, w, r.h, 2.0, edge);
                }
                gfx::text(name, r.x + 10.0, r.y + 4.0, 13.0, with_alpha(col, fade));
            }
            Item::Note(text) => {
                let c = with_alpha(token_color(TokKind::Comment), 0.9 * fade);
                gfx::text(&clip(&format!("; {text}"), 12.0, r.w - 8.0), r.x + 4.0, r.y + 2.0, 12.0, c);
            }
            Item::Gap => {}
        }
        chips
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dronoids::programs::BUILTINS;

    #[test]
    fn tiles_read_back_as_the_same_program() {
        for b in BUILTINS {
            let items: Vec<Item> = parse(b.source).into_iter().map(|p| p.0).collect();
            let again = render(&items);
            let a = vm::assemble("a", b.source).unwrap();
            let c = vm::assemble("c", &again).unwrap();
            assert_eq!(a.code, c.code, "{}", b.name);
            // And once written by the tiles, it stays as it is.
            let items2: Vec<Item> = parse(&again).into_iter().map(|p| p.0).collect();
            assert_eq!(render(&items2), again, "{}", b.name);
        }
    }

    #[test]
    fn labels_on_the_line_of_an_instruction_become_tiles_of_their_own() {
        let items: Vec<Item> = parse("loop: MOVE N ; go north\n\n; a note\nend: x: WAIT").into_iter().map(|p| p.0).collect();
        assert_eq!(
            items,
            vec![
                Item::Label("loop".into()),
                Item::Op { op: "MOVE".into(), args: vec!["N".into()], note: "go north".into() },
                Item::Gap,
                Item::Note("a note".into()),
                Item::Label("end".into()),
                Item::Label("x".into()),
                Item::Op { op: "WAIT".into(), args: vec![], note: String::new() },
            ]
        );
    }

    #[test]
    fn new_tiles_come_with_operands() {
        let items = vec![Item::Label("here".into())];
        assert_eq!(
            Kind::Op("IFNEAR").make(&items),
            Item::Op { op: "IFNEAR".into(), args: vec!["ORE".into(), "here".into()], note: String::new() }
        );
        assert_eq!(Kind::Label.make(&items), Item::Label("spot1".into()));
        assert_eq!(Kind::Op("JUMP").make(&[]), Item::Op { op: "JUMP".into(), args: vec!["start".into()], note: String::new() });
        assert!(choices(Arg::L, &labels_of(&items)) == vec!["here".to_string()]);
    }
}
