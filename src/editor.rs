//! A small code editor for DASM: syntax highlighting, selection, clipboard,
//! undo/redo, error markers and a "current instruction" marker for the
//! drone debugger.

use macroquad::miniquad::window::{clipboard_get, clipboard_set};
use macroquad::prelude::*;

use dronoids::vm::{self, AsmError, Cat, TokKind};

use crate::gfx::{self, pal, with_alpha};
use crate::ui::{Typed, Ui};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pos {
    pub line: usize,
    pub col: usize,
}

struct Snapshot {
    lines: Vec<String>,
    cur: Pos,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditKind {
    None,
    Typing,
    Other,
}

pub struct Editor {
    pub lines: Vec<String>,
    pub cur: Pos,
    pub anchor: Option<Pos>,
    pub scroll: f32,
    pub focused: bool,
    pub read_only: bool,
    pub errors: Vec<AsmError>,
    pub exec_line: Option<usize>,
    pub dirty: bool,
    pub font_size: f32,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    last_kind: EditKind,
    dragging: bool,
    /// A finger landed in the code: the scroll and finger height back then.
    touch_scroll: Option<(f32, f32)>,
    blink: f32,
    follow_cursor: bool,
}

pub fn token_color(k: TokKind) -> Color {
    match k {
        TokKind::Op(Cat::Action) => Color::from_hex(0xff5fd2),
        TokKind::Op(Cat::Sense) => Color::from_hex(0x7dff6a),
        TokKind::Op(Cat::Flow) => Color::from_hex(0xb48cff),
        TokKind::Op(Cat::Data) => Color::from_hex(0x3ce8ff),
        TokKind::Reg => Color::from_hex(0xffd166),
        TokKind::Const => Color::from_hex(0xff9f53),
        TokKind::Number => Color::from_hex(0xffb88a),
        TokKind::Label => Color::from_hex(0xd6c2ff),
        TokKind::LabelDef => Color::from_hex(0xfff2a8),
        TokKind::Comment => Color::from_hex(0x56679a),
        TokKind::Punct => Color::from_hex(0x7c8bb5),
        TokKind::Unknown => pal::BAD,
    }
}

fn byte_of(s: &str, col: usize) -> usize {
    s.char_indices().nth(col).map(|(i, _)| i).unwrap_or(s.len())
}

fn char_len(s: &str) -> usize {
    s.chars().count()
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl Editor {
    pub fn new() -> Editor {
        Editor {
            lines: vec![String::new()],
            cur: Pos::default(),
            anchor: None,
            scroll: 0.0,
            focused: false,
            read_only: false,
            errors: vec![],
            exec_line: None,
            dirty: false,
            font_size: 16.0,
            undo: vec![],
            redo: vec![],
            last_kind: EditKind::None,
            dragging: false,
            touch_scroll: None,
            blink: 0.0,
            follow_cursor: false,
        }
    }

    pub fn set_text(&mut self, s: &str) {
        self.lines = s.split('\n').map(|l| l.trim_end_matches('\r').to_string()).collect();
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cur = Pos::default();
        self.anchor = None;
        self.scroll = 0.0;
        self.undo.clear();
        self.redo.clear();
        self.last_kind = EditKind::None;
        self.dirty = false;
        self.errors.clear();
    }

    pub fn text(&self) -> String {
        let mut s = self.lines.join("\n");
        if !s.ends_with('\n') {
            s.push('\n');
        }
        s
    }

    // ----- primitive edits ----------------------------------------------

    fn snapshot(&mut self, kind: EditKind) {
        if kind == EditKind::Typing && self.last_kind == EditKind::Typing {
            return;
        }
        self.undo.push(Snapshot { lines: self.lines.clone(), cur: self.cur });
        if self.undo.len() > 300 {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.last_kind = kind;
    }

    fn clamp(&self, p: Pos) -> Pos {
        let line = p.line.min(self.lines.len() - 1);
        Pos { line, col: p.col.min(char_len(&self.lines[line])) }
    }

    pub fn selection(&self) -> Option<(Pos, Pos)> {
        let a = self.anchor?;
        if a == self.cur {
            return None;
        }
        Some(if a < self.cur { (a, self.cur) } else { (self.cur, a) })
    }

    fn selected_text(&self) -> Option<String> {
        let (a, b) = self.selection()?;
        if a.line == b.line {
            let l = &self.lines[a.line];
            return Some(l[byte_of(l, a.col)..byte_of(l, b.col)].to_string());
        }
        let mut out = String::new();
        let first = &self.lines[a.line];
        out.push_str(&first[byte_of(first, a.col)..]);
        for l in &self.lines[a.line + 1..b.line] {
            out.push('\n');
            out.push_str(l);
        }
        out.push('\n');
        let last = &self.lines[b.line];
        out.push_str(&last[..byte_of(last, b.col)]);
        Some(out)
    }

    fn delete_selection(&mut self) -> bool {
        let Some((a, b)) = self.selection() else {
            self.anchor = None;
            return false;
        };
        let tail = {
            let l = &self.lines[b.line];
            l[byte_of(l, b.col)..].to_string()
        };
        let head_end = byte_of(&self.lines[a.line], a.col);
        self.lines[a.line].truncate(head_end);
        self.lines[a.line].push_str(&tail);
        self.lines.drain(a.line + 1..=b.line);
        self.cur = a;
        self.anchor = None;
        true
    }

    fn insert(&mut self, s: &str) {
        self.delete_selection();
        let line = &self.lines[self.cur.line];
        let at = byte_of(line, self.cur.col);
        let tail = line[at..].to_string();
        self.lines[self.cur.line].truncate(at);
        let mut parts = s.split('\n');
        let first = parts.next().unwrap_or("");
        self.lines[self.cur.line].push_str(first);
        let mut line_idx = self.cur.line;
        let mut col = char_len(&self.lines[line_idx]);
        for p in parts {
            line_idx += 1;
            let p = p.trim_end_matches('\r');
            self.lines.insert(line_idx, p.to_string());
            col = char_len(p);
        }
        self.lines[line_idx].push_str(&tail);
        self.cur = Pos { line: line_idx, col };
        self.dirty = true;
        self.follow_cursor = true;
    }

    fn backspace(&mut self) {
        if self.delete_selection() {
            self.dirty = true;
            return;
        }
        let Pos { line, col } = self.cur;
        if col > 0 {
            let l = &self.lines[line];
            // Smart unindent inside leading spaces.
            let before: String = l.chars().take(col).collect();
            let n = if before.chars().all(|c| c == ' ') && col >= 4 { (col - 1) % 4 + 1 } else { 1 };
            let start = byte_of(l, col - n);
            let end = byte_of(l, col);
            self.lines[line].replace_range(start..end, "");
            self.cur.col -= n;
        } else if line > 0 {
            let cur_line = self.lines.remove(line);
            let prev_len = char_len(&self.lines[line - 1]);
            self.lines[line - 1].push_str(&cur_line);
            self.cur = Pos { line: line - 1, col: prev_len };
        }
        self.dirty = true;
        self.follow_cursor = true;
    }

    fn delete(&mut self) {
        if self.delete_selection() {
            self.dirty = true;
            return;
        }
        let Pos { line, col } = self.cur;
        let len = char_len(&self.lines[line]);
        if col < len {
            let l = &self.lines[line];
            let (s, e) = (byte_of(l, col), byte_of(l, col + 1));
            self.lines[line].replace_range(s..e, "");
        } else if line + 1 < self.lines.len() {
            let next = self.lines.remove(line + 1);
            self.lines[line].push_str(&next);
        }
        self.dirty = true;
    }

    fn newline(&mut self) {
        let line = &self.lines[self.cur.line];
        let indent: String = line.chars().take_while(|c| *c == ' ').collect();
        let code = vm::strip_comment(line).trim_end();
        let extra = if code.ends_with(':') && indent.is_empty() { "    " } else { "" };
        self.insert(&format!("\n{indent}{extra}"));
    }

    fn indent_lines(&mut self, out: bool) {
        let (a, b) = self.selection().unwrap_or((self.cur, self.cur));
        for l in a.line..=b.line {
            if out {
                let n = self.lines[l].chars().take(4).take_while(|c| *c == ' ').count();
                self.lines[l].replace_range(0..n, "");
            } else {
                self.lines[l].insert_str(0, "    ");
            }
        }
        self.dirty = true;
        if let Some(an) = self.anchor {
            self.anchor = Some(self.clamp(an));
        }
        self.cur = self.clamp(self.cur);
    }

    fn word_left(&self, p: Pos) -> Pos {
        if p.col == 0 {
            return if p.line > 0 { Pos { line: p.line - 1, col: char_len(&self.lines[p.line - 1]) } } else { p };
        }
        let chars: Vec<char> = self.lines[p.line].chars().collect();
        let mut c = p.col;
        while c > 0 && !is_word(chars[c - 1]) {
            c -= 1;
        }
        while c > 0 && is_word(chars[c - 1]) {
            c -= 1;
        }
        Pos { line: p.line, col: c }
    }

    fn word_right(&self, p: Pos) -> Pos {
        let chars: Vec<char> = self.lines[p.line].chars().collect();
        if p.col >= chars.len() {
            return if p.line + 1 < self.lines.len() { Pos { line: p.line + 1, col: 0 } } else { p };
        }
        let mut c = p.col;
        while c < chars.len() && !is_word(chars[c]) {
            c += 1;
        }
        while c < chars.len() && is_word(chars[c]) {
            c += 1;
        }
        Pos { line: p.line, col: c }
    }

    fn select_word_at(&mut self, p: Pos) {
        let chars: Vec<char> = self.lines[p.line].chars().collect();
        let (mut s, mut e) = (p.col.min(chars.len()), p.col.min(chars.len()));
        while s > 0 && is_word(chars[s - 1]) {
            s -= 1;
        }
        while e < chars.len() && is_word(chars[e]) {
            e += 1;
        }
        self.anchor = Some(Pos { line: p.line, col: s });
        self.cur = Pos { line: p.line, col: e };
    }

    fn do_undo(&mut self) {
        if let Some(s) = self.undo.pop() {
            self.redo.push(Snapshot { lines: std::mem::replace(&mut self.lines, s.lines), cur: self.cur });
            self.cur = self.clamp(s.cur);
            self.anchor = None;
            self.dirty = true;
            self.last_kind = EditKind::None;
        }
    }

    fn do_redo(&mut self) {
        if let Some(s) = self.redo.pop() {
            self.undo.push(Snapshot { lines: std::mem::replace(&mut self.lines, s.lines), cur: self.cur });
            self.cur = self.clamp(s.cur);
            self.anchor = None;
            self.dirty = true;
            self.last_kind = EditKind::None;
        }
    }

    // ----- layout ---------------------------------------------------------

    fn line_h(&self) -> f32 {
        (self.font_size * 1.35).round()
    }

    fn gutter_w(&self) -> f32 {
        gfx::char_width(self.font_size) * 3.0 + 26.0
    }

    fn visible_lines(&self, r: Rect) -> usize {
        ((r.h - 8.0) / self.line_h()).floor().max(1.0) as usize
    }

    fn pos_at(&self, r: Rect, m: Vec2) -> Pos {
        let lh = self.line_h();
        let line = ((m.y - r.y - 4.0) / lh + self.scroll.floor()).floor().max(0.0) as usize;
        let cw = gfx::char_width(self.font_size);
        let col = ((m.x - r.x - self.gutter_w() - 6.0) / cw).round().max(0.0) as usize;
        self.clamp(Pos { line, col })
    }

    fn ensure_visible(&mut self, r: Rect) {
        let vis = self.visible_lines(r) as f32;
        let l = self.cur.line as f32;
        if l < self.scroll {
            self.scroll = l;
        } else if l >= self.scroll + vis {
            self.scroll = l - vis + 1.0;
        }
    }

    // ----- input ------------------------------------------------------------

    /// Handle input. Returns true when the user asks to apply (Ctrl+S).
    pub fn update(&mut self, ui: &mut Ui, r: Rect) -> bool {
        self.blink += get_frame_time();
        let max_scroll = (self.lines.len() as f32 - 3.0).max(0.0);

        if ui.hover(r) && ui.wheel != 0.0 {
            self.scroll = (self.scroll - ui.wheel.signum() * 3.0).clamp(0.0, max_scroll);
        }

        if ui.touch {
            // A finger slides the code; a tap puts the cursor there.
            if ui.pressed {
                if r.contains(ui.mouse) {
                    ui.pressed = false;
                    self.touch_scroll = Some((self.scroll, ui.mouse.y));
                    if !self.read_only {
                        ui.text_touched = true;
                    }
                } else {
                    self.focused = false;
                }
            }
            if let Some((from, y0)) = self.touch_scroll {
                if ui.drag.is_some() {
                    self.scroll = (from - (ui.mouse.y - y0) / self.line_h()).clamp(0.0, max_scroll);
                }
                if ui.tap {
                    self.focused = true;
                    let p = self.pos_at(r, ui.mouse);
                    if ui.double_click {
                        self.select_word_at(p);
                    } else {
                        self.anchor = None;
                        self.cur = p;
                    }
                    self.blink = 0.0;
                }
                if !ui.down {
                    self.touch_scroll = None;
                }
            }
        } else if ui.pressed {
            if r.contains(ui.mouse) {
                ui.pressed = false;
                self.focused = true;
                let p = self.pos_at(r, ui.mouse);
                if ui.double_click {
                    self.select_word_at(p);
                } else {
                    if ui.shift {
                        self.anchor.get_or_insert(self.cur);
                    } else {
                        self.anchor = Some(p);
                    }
                    self.cur = p;
                    self.dragging = true;
                }
                self.blink = 0.0;
            } else {
                self.focused = false;
            }
        }
        if self.dragging {
            if ui.down {
                let mut p = ui.mouse;
                // Auto-scroll when dragging past the edges.
                if p.y < r.y {
                    self.scroll = (self.scroll - 0.5).max(0.0);
                    p.y = r.y;
                } else if p.y > r.y + r.h {
                    self.scroll = (self.scroll + 0.5).min(max_scroll);
                    p.y = r.y + r.h - 1.0;
                }
                self.cur = self.pos_at(r, p);
            } else {
                self.dragging = false;
            }
        }

        if !self.focused {
            return false;
        }
        ui.keyboard_taken = true;
        let mut apply = false;
        let editable = !self.read_only;
        let shift = ui.shift;
        let ctrl = ui.ctrl;

        // Text input.
        if editable && !ctrl {
            let chars: Vec<char> = ui.chars.iter().copied().filter(|c| !c.is_control()).collect();
            for c in chars {
                self.snapshot(if c == ' ' { EditKind::Other } else { EditKind::Typing });
                self.insert(&c.to_string());
                self.blink = 0.0;
            }
        }
        // The phone's keyboard.
        if editable {
            ui.text_focus = true;
            for t in ui.typed.clone() {
                match t {
                    Typed::Char(c) if !c.is_control() => {
                        self.snapshot(if c == ' ' { EditKind::Other } else { EditKind::Typing });
                        self.insert(&c.to_string());
                    }
                    Typed::Char(_) => {}
                    Typed::Back => {
                        self.snapshot(EditKind::Other);
                        self.backspace();
                    }
                    Typed::Enter => {
                        self.snapshot(EditKind::Other);
                        self.newline();
                    }
                }
                self.blink = 0.0;
                self.follow_cursor = true;
            }
        }

        let mut moved = false;
        let mut move_to = |ed: &mut Editor, p: Pos| {
            if shift {
                ed.anchor.get_or_insert(ed.cur);
            } else {
                ed.anchor = None;
            }
            ed.cur = ed.clamp(p);
            ed.last_kind = EditKind::None;
            moved = true;
        };

        let c = self.cur;
        if ui.rep(KeyCode::Left) {
            let p = if !shift && self.selection().is_some() {
                self.selection().unwrap().0
            } else if ctrl {
                self.word_left(c)
            } else if c.col > 0 {
                Pos { col: c.col - 1, ..c }
            } else if c.line > 0 {
                Pos { line: c.line - 1, col: char_len(&self.lines[c.line - 1]) }
            } else {
                c
            };
            move_to(self, p);
        }
        if ui.rep(KeyCode::Right) {
            let len = char_len(&self.lines[c.line]);
            let p = if !shift && self.selection().is_some() {
                self.selection().unwrap().1
            } else if ctrl {
                self.word_right(c)
            } else if c.col < len {
                Pos { col: c.col + 1, ..c }
            } else if c.line + 1 < self.lines.len() {
                Pos { line: c.line + 1, col: 0 }
            } else {
                c
            };
            move_to(self, p);
        }
        if ui.rep(KeyCode::Up) && c.line > 0 {
            move_to(self, Pos { line: c.line - 1, col: c.col });
        }
        if ui.rep(KeyCode::Down) {
            move_to(self, Pos { line: c.line + 1, col: c.col });
        }
        if ui.rep(KeyCode::Home) {
            let indent = self.lines[c.line].chars().take_while(|ch| *ch == ' ').count();
            let col = if ctrl || c.col == indent { 0 } else { indent };
            move_to(self, Pos { line: if ctrl { 0 } else { c.line }, col });
        }
        if ui.rep(KeyCode::End) {
            let line = if ctrl { self.lines.len() - 1 } else { c.line };
            move_to(self, Pos { line, col: usize::MAX });
        }
        let page = self.visible_lines(r).saturating_sub(1).max(1);
        if ui.rep(KeyCode::PageUp) {
            move_to(self, Pos { line: c.line.saturating_sub(page), col: c.col });
        }
        if ui.rep(KeyCode::PageDown) {
            move_to(self, Pos { line: c.line + page, col: c.col });
        }

        if ctrl && is_key_pressed(KeyCode::A) {
            self.anchor = Some(Pos::default());
            let last = self.lines.len() - 1;
            self.cur = Pos { line: last, col: char_len(&self.lines[last]) };
        }
        if ctrl && (is_key_pressed(KeyCode::C) || is_key_pressed(KeyCode::X)) {
            let text = self.selected_text().unwrap_or_else(|| format!("{}\n", self.lines[c.line]));
            clipboard_set(&text);
            if editable && is_key_pressed(KeyCode::X) {
                self.snapshot(EditKind::Other);
                if self.selection().is_some() {
                    self.delete_selection();
                } else if self.lines.len() > 1 {
                    self.lines.remove(c.line);
                    self.cur = self.clamp(Pos { line: c.line, col: 0 });
                } else {
                    self.lines[0].clear();
                    self.cur = Pos::default();
                }
                self.dirty = true;
            }
        }
        if ctrl && is_key_pressed(KeyCode::S) {
            apply = true;
        }

        if editable {
            if ctrl && is_key_pressed(KeyCode::V)
                && let Some(s) = clipboard_get() {
                    self.snapshot(EditKind::Other);
                    let s = s.replace("\r\n", "\n").replace('\t', "    ");
                    self.insert(&s);
                }
            if ctrl && ui.rep(KeyCode::Z) {
                if shift { self.do_redo() } else { self.do_undo() }
            }
            if ctrl && ui.rep(KeyCode::Y) {
                self.do_redo();
            }
            if ctrl && is_key_pressed(KeyCode::D) {
                self.snapshot(EditKind::Other);
                let l = self.lines[c.line].clone();
                self.lines.insert(c.line + 1, l);
                self.cur.line += 1;
                self.dirty = true;
            }
            if ui.rep(KeyCode::Enter) || ui.rep(KeyCode::KpEnter) {
                self.snapshot(EditKind::Other);
                self.newline();
            }
            if ui.rep(KeyCode::Tab) {
                self.snapshot(EditKind::Other);
                if shift || self.selection().is_some_and(|(a, b)| a.line != b.line) {
                    self.indent_lines(shift);
                } else {
                    let n = 4 - self.cur.col % 4;
                    self.insert(&" ".repeat(n));
                }
            }
            if ui.rep(KeyCode::Backspace) {
                self.snapshot(EditKind::Other);
                self.backspace();
            }
            if ui.rep(KeyCode::Delete) {
                self.snapshot(EditKind::Other);
                self.delete();
            }
        }

        if moved || self.follow_cursor {
            self.blink = 0.0;
            self.ensure_visible(r);
            self.follow_cursor = false;
        }
        self.cur = self.clamp(self.cur);
        apply
    }

    /// Scroll so that `line` is visible (debugger).
    pub fn reveal(&mut self, r: Rect, line: usize) {
        let vis = self.visible_lines(r) as f32;
        let l = line as f32;
        if l < self.scroll + 1.0 || l > self.scroll + vis - 2.0 {
            self.scroll = (l - vis / 2.0).max(0.0);
        }
    }

    // ----- drawing --------------------------------------------------------

    pub fn draw(&self, ui: &mut Ui, r: Rect) {
        draw_rectangle(r.x, r.y, r.w, r.h, Color::new(0.008, 0.012, 0.03, 0.97));
        let lh = self.line_h();
        let fs = self.font_size;
        let cw = gfx::char_width(fs);
        let gw = self.gutter_w();
        let tx = r.x + gw + 6.0;
        let max_cols = ((r.x + r.w - tx - 14.0) / cw).floor().max(1.0) as usize;
        draw_rectangle(r.x, r.y, gw, r.h, Color::new(0.03, 0.04, 0.09, 1.0));
        draw_line(r.x + gw, r.y, r.x + gw, r.y + r.h, 1.0, pal::FAINT);

        let first = self.scroll.floor() as usize;
        let vis = self.visible_lines(r);
        let sel = self.selection();
        let mut hover_tip: Option<String> = None;

        for (row, li) in (first..self.lines.len().min(first + vis)).enumerate() {
            let y = r.y + 4.0 + row as f32 * lh;
            let line = &self.lines[li];
            let err = self.errors.iter().find(|e| e.line == li);

            if self.focused && li == self.cur.line && sel.is_none() {
                draw_rectangle(r.x + gw, y, r.w - gw, lh, with_alpha(pal::ACCENT, 0.05));
            }
            if err.is_some() {
                draw_rectangle(r.x + gw, y, r.w - gw, lh, with_alpha(pal::BAD, 0.12));
            }
            if self.exec_line == Some(li) {
                let pulse = 0.18 + 0.07 * (ui.time * 6.0).sin();
                draw_rectangle(r.x + gw, y, r.w - gw, lh, with_alpha(pal::ACCENT, pulse));
                draw_rectangle(r.x + gw, y, 3.0, lh, pal::ACCENT);
                let cy = y + lh / 2.0;
                draw_triangle(
                    vec2(r.x + 4.0, cy - 5.0),
                    vec2(r.x + 4.0, cy + 5.0),
                    vec2(r.x + 12.0, cy),
                    pal::ACCENT,
                );
            }
            // Selection.
            if let Some((a, b)) = sel
                && li >= a.line && li <= b.line {
                    let s = if li == a.line { a.col } else { 0 };
                    let e = if li == b.line { b.col } else { char_len(line) + 1 };
                    let (s, e) = (s.min(max_cols), e.min(max_cols + 1));
                    if e > s {
                        draw_rectangle(tx + s as f32 * cw, y, (e - s) as f32 * cw, lh, with_alpha(pal::VIOLET, 0.35));
                    }
                }
            // Line number.
            let num = format!("{:>3}", li + 1);
            let nc = if err.is_some() {
                pal::BAD
            } else if li == self.cur.line && self.focused {
                pal::TEXT
            } else {
                pal::FAINT
            };
            gfx::text(&num, r.x + 14.0, y + (lh - fs * 0.9) / 2.0, fs * 0.9, nc);
            // Tokens.
            let ty = y + (lh - fs) / 2.0;
            for (s, e, kind) in vm::highlight(line) {
                let col = line[..s].chars().count();
                if col >= max_cols {
                    break;
                }
                let word = &line[s..e];
                let n = char_len(word).min(max_cols - col);
                let shown: String = word.chars().take(n).collect();
                let x = tx + col as f32 * cw;
                gfx::text(&shown, x, ty, fs, token_color(kind));
                if let TokKind::Op(_) = kind {
                    let hr = Rect::new(x, y, n as f32 * cw, lh);
                    if hr.contains(ui.mouse)
                        && let Some(m) = vm::mnemonic(word) {
                            hover_tip = Some(format!("{}  —  {}", m.syntax, m.help));
                        }
                }
            }
            if let Some(e) = err {
                let er = Rect::new(r.x, y, r.w, lh);
                if er.contains(ui.mouse) {
                    hover_tip = Some(e.msg.clone());
                }
            }
        }

        // Cursor.
        if self.focused && (self.blink * 1.8).fract() < 0.6 {
            let li = self.cur.line;
            if li >= first && li < first + vis {
                let y = r.y + 4.0 + (li - first) as f32 * lh;
                let x = tx + self.cur.col.min(max_cols) as f32 * cw;
                draw_rectangle(x - 1.0, y + 2.0, 2.0, lh - 4.0, pal::ACCENT);
            }
        }

        // Scrollbar.
        let total = self.lines.len().max(1) as f32;
        if total > vis as f32 {
            let h = r.h * (vis as f32 / total);
            let y = r.y + (r.h - h) * (self.scroll / (total - vis as f32).max(1.0)).min(1.0);
            draw_rectangle(r.x + r.w - 5.0, y, 3.0, h, with_alpha(pal::ACCENT, 0.4));
        }
        let edge = if self.focused { with_alpha(pal::ACCENT, 0.7) } else { pal::FAINT };
        draw_rectangle_lines(r.x, r.y, r.w, r.h, 1.0, edge);
        if let Some(t) = hover_tip {
            ui.tooltip(&t);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_delete_and_selection() {
        let mut e = Editor::new();
        e.set_text("abc\ndef");
        e.cur = Pos { line: 0, col: 1 };
        e.insert("X\nY");
        assert_eq!(e.lines, vec!["aX", "Ybc", "def"]);
        e.anchor = Some(Pos { line: 0, col: 1 });
        e.cur = Pos { line: 2, col: 1 };
        assert_eq!(e.selected_text().unwrap(), "X\nYbc\nd");
        e.delete_selection();
        assert_eq!(e.lines, vec!["aef"]);
        e.cur = Pos { line: 0, col: 0 };
        e.backspace();
        assert_eq!(e.lines, vec!["aef"]);
        e.cur = Pos { line: 0, col: 3 };
        e.backspace();
        assert_eq!(e.lines, vec!["ae"]);
    }

    #[test]
    fn smart_unindent_and_newline_indent() {
        let mut e = Editor::new();
        e.set_text("loop:");
        e.cur = Pos { line: 0, col: 5 };
        e.newline();
        assert_eq!(e.lines, vec!["loop:", "    "]);
        e.backspace();
        assert_eq!(e.lines, vec!["loop:", ""]);
    }
}
