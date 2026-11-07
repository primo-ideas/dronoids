//! A tiny immediate-mode UI with a neon look.

use std::collections::HashMap;

use macroquad::prelude::*;

use crate::gfx::{self, mix, pal, with_alpha};

const REPEATABLE: &[KeyCode] = &[
    KeyCode::Backspace,
    KeyCode::Delete,
    KeyCode::Left,
    KeyCode::Right,
    KeyCode::Up,
    KeyCode::Down,
    KeyCode::Enter,
    KeyCode::KpEnter,
    KeyCode::Home,
    KeyCode::End,
    KeyCode::PageUp,
    KeyCode::PageDown,
    KeyCode::Tab,
    KeyCode::Z,
    KeyCode::Y,
];

/// A finger that moved less than this from where it landed is still a tap.
const TAP_SLOP: f32 = 12.0;
/// Where the pointer goes when no finger is on the screen (no hover).
const NOWHERE: Vec2 = vec2(-10_000.0, -10_000.0);

/// Two fingers on the screen this frame.
#[derive(Clone, Copy, Debug)]
pub struct Pinch {
    /// Midpoint between the two fingers.
    pub mid: Vec2,
    /// How far that midpoint moved since last frame.
    pub moved: Vec2,
    /// Distance between the fingers relative to last frame (above 1: apart).
    pub scale: f32,
}

/// Input from the phone's on-screen keyboard, in the order it was typed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Typed {
    Char(char),
    Back,
    Enter,
}

#[derive(Default)]
struct Fingers {
    /// Fingers on the screen: id, where it landed, where it is now.
    down: Vec<(u64, Vec2, Vec2)>,
    /// The finger that acts as the mouse.
    primary: Option<u64>,
    /// The primary finger went too far to be a tap.
    moved: bool,
    /// Another finger joined (or the touch was cancelled): no tap.
    multi: bool,
    /// Last frame's pair of fingers, their midpoint and spread.
    pair: Option<(u64, u64, Vec2, f32)>,
    last_tap: f32,
    last_tap_at: Vec2,
}

/// Small screens (a phone held upright) get a rearranged interface.
pub fn compact() -> bool {
    screen_width() < 700.0
}

#[derive(Default)]
pub struct Ui {
    pub mouse: Vec2,
    /// Left click this frame, not yet consumed by a widget.
    pub pressed: bool,
    pub down: bool,
    pub released: bool,
    pub right_pressed: bool,
    pub wheel: f32,
    pub double_click: bool,
    pub chars: Vec<char>,
    pub ctrl: bool,
    pub shift: bool,
    /// Rectangles covered by UI this frame (world input ignores them).
    blockers: Vec<Rect>,
    prev_blockers: Vec<Rect>,
    held: HashMap<KeyCode, (f32, f32)>,
    fired: Vec<KeyCode>,
    last_click: f32,
    pub time: f32,
    tooltip: Option<String>,
    /// Some widget wants the keyboard this frame (disables hotkeys).
    pub keyboard_taken: bool,
    /// The last input came from a finger rather than a mouse.
    pub touch: bool,
    /// A finger was lifted without moving: a tap at `mouse`.
    pub tap: bool,
    /// The finger is sliding: how far it moved this frame.
    pub drag: Option<Vec2>,
    /// Two fingers are on the screen.
    pub pinch: Option<Pinch>,
    /// Typed on the on-screen keyboard since last frame.
    pub typed: Vec<Typed>,
    /// An editable text widget has the focus this frame.
    pub text_focus: bool,
    /// An editable text widget was touched this frame: bring up the keyboard.
    pub text_touched: bool,
    fingers: Fingers,
    last_mouse: Vec2,
    keyboard_up: bool,
}

impl Ui {
    pub fn begin_frame(&mut self) {
        let dt = get_frame_time();
        self.time += dt;
        let m: Vec2 = mouse_position().into();
        let wheel = mouse_wheel().1;
        let clicked = [MouseButton::Left, MouseButton::Right, MouseButton::Middle].into_iter().any(is_mouse_button_pressed);
        if m != self.last_mouse || wheel != 0.0 || clicked {
            self.touch = false;
        }
        self.last_mouse = m;
        self.double_click = false;
        self.tap = false;
        self.drag = None;
        self.pinch = None;
        let fingers = self.read_fingers();
        if !fingers && !self.touch {
            self.mouse = m;
            self.pressed = is_mouse_button_pressed(MouseButton::Left);
            self.down = is_mouse_button_down(MouseButton::Left);
            self.released = is_mouse_button_released(MouseButton::Left);
            self.right_pressed = is_mouse_button_pressed(MouseButton::Right);
            self.wheel = wheel;
            if self.pressed {
                self.double_click = self.time - self.last_click < 0.3;
                self.last_click = self.time;
            }
        }
        self.typed.clear();
        for c in crate::platform::typed().chars() {
            self.typed.push(match c {
                '\u{8}' => Typed::Back,
                '\n' | '\r' => Typed::Enter,
                c => Typed::Char(c),
            });
        }
        self.text_focus = false;
        self.text_touched = false;
        self.ctrl = is_key_down(KeyCode::LeftControl) || is_key_down(KeyCode::RightControl);
        self.shift = is_key_down(KeyCode::LeftShift) || is_key_down(KeyCode::RightShift);
        self.chars.clear();
        while let Some(c) = get_char_pressed() {
            self.chars.push(c);
        }
        self.prev_blockers = std::mem::take(&mut self.blockers);
        self.tooltip = None;
        self.keyboard_taken = false;

        self.fired.clear();
        for &k in REPEATABLE {
            if is_key_down(k) {
                const DELAY: f32 = 0.38;
                const RATE: f32 = 0.032;
                match self.held.get_mut(&k) {
                    None => {
                        self.held.insert(k, (0.0, DELAY));
                        self.fired.push(k);
                    }
                    Some((t, next)) => {
                        *t += dt;
                        if *t >= *next {
                            *next = *t + RATE;
                            self.fired.push(k);
                        }
                    }
                }
            } else {
                self.held.remove(&k);
            }
        }
    }

    /// Fingers drive the pointer: the first finger down is the mouse, a
    /// tap is a click, sliding drags, a second finger makes a pinch.
    /// Returns whether any finger was involved this frame.
    fn read_fingers(&mut self) -> bool {
        let list = touches();
        if list.is_empty() && self.fingers.down.is_empty() && self.fingers.primary.is_none() {
            if self.touch {
                // Nothing under the finger any more: no hover either.
                self.pressed = false;
                self.released = false;
                self.down = false;
                self.right_pressed = false;
                self.wheel = 0.0;
                self.mouse = NOWHERE;
            }
            return false;
        }
        self.touch = true;
        let f = &mut self.fingers;
        let (mut pressed, mut released) = (false, false);
        let mut at = None;
        for t in &list {
            let known = f.down.iter().position(|d| d.0 == t.id);
            match t.phase {
                TouchPhase::Started | TouchPhase::Moved | TouchPhase::Stationary => match known {
                    Some(i) => f.down[i].2 = t.position,
                    None => {
                        if f.down.is_empty() && f.primary.is_none() {
                            f.primary = Some(t.id);
                            f.moved = false;
                            f.multi = false;
                            pressed = true;
                            at = Some(t.position);
                        } else {
                            f.multi = true;
                        }
                        f.down.push((t.id, t.position, t.position));
                    }
                },
                TouchPhase::Ended | TouchPhase::Cancelled => {
                    match known {
                        Some(i) => {
                            f.down.remove(i);
                        }
                        // Down and up again between two frames: still a tap.
                        None if f.down.is_empty() && f.primary.is_none() => {
                            f.primary = Some(t.id);
                            f.moved = false;
                            f.multi = false;
                            pressed = true;
                        }
                        None => {}
                    }
                    if t.phase == TouchPhase::Cancelled {
                        f.multi = true;
                    }
                    if f.primary == Some(t.id) {
                        f.primary = None;
                        released = true;
                        at = Some(t.position);
                    }
                }
            }
        }
        if let Some(id) = f.primary
            && let Some(&(_, start, now)) = f.down.iter().find(|d| d.0 == id)
        {
            if !f.multi {
                if f.moved {
                    self.drag = Some(now - self.mouse);
                } else if now.distance(start) > TAP_SLOP {
                    f.moved = true;
                    self.drag = Some(now - start);
                }
            }
            at = Some(now);
        }
        f.pair = match f.down.as_slice() {
            [a, b, ..] => {
                let mid = (a.2 + b.2) / 2.0;
                let spread = a.2.distance(b.2).max(1.0);
                if let Some((ia, ib, last_mid, last_spread)) = f.pair
                    && ia == a.0
                    && ib == b.0
                {
                    self.pinch = Some(Pinch { mid, moved: mid - last_mid, scale: spread / last_spread });
                }
                Some((a.0, b.0, mid, spread))
            }
            _ => None,
        };
        self.pressed = pressed;
        self.released = released;
        self.down = f.primary.is_some();
        self.right_pressed = false;
        self.wheel = 0.0;
        self.mouse = at.unwrap_or(NOWHERE);
        if released && !f.moved && !f.multi {
            self.tap = true;
            self.double_click = self.time - f.last_tap < 0.35 && self.mouse.distance(f.last_tap_at) < 40.0;
            f.last_tap = if self.double_click { -1.0 } else { self.time };
            f.last_tap_at = self.mouse;
        }
        true
    }

    /// Bring the phone's keyboard up when a text widget is touched, and put
    /// it away once nothing is being typed into.
    fn sync_keyboard(&mut self) {
        if self.touch && self.text_touched {
            crate::platform::keyboard(true);
            self.keyboard_up = true;
        } else if self.keyboard_up && !self.text_focus && !self.down {
            crate::platform::keyboard(false);
            self.keyboard_up = false;
        }
    }

    /// Key pressed, with auto-repeat while held.
    pub fn rep(&self, k: KeyCode) -> bool {
        self.fired.contains(&k)
    }

    pub fn block(&mut self, r: Rect) {
        self.blockers.push(r);
    }

    /// Is the mouse over UI drawn this frame (or last frame, for queries
    /// made before the UI is drawn)?
    pub fn over_ui(&self) -> bool {
        self.blockers.iter().chain(self.prev_blockers.iter()).any(|r| r.contains(self.mouse))
    }

    pub fn hover(&self, r: Rect) -> bool {
        r.contains(self.mouse)
    }

    /// Consume a click inside `r`.
    pub fn click(&mut self, r: Rect) -> bool {
        if self.pressed && r.contains(self.mouse) {
            self.pressed = false;
            true
        } else {
            false
        }
    }

    pub fn tooltip(&mut self, s: &str) {
        self.tooltip = Some(s.to_string());
    }

    pub fn panel(&mut self, r: Rect) {
        self.block(r);
        panel(r, pal::PANEL_EDGE);
    }

    pub fn button(&mut self, r: Rect, label: &str) -> bool {
        self.button_ex(r, label, false, true, pal::ACCENT)
    }

    pub fn button_ex(&mut self, r: Rect, label: &str, active: bool, enabled: bool, color: Color) -> bool {
        self.block(r);
        let hover = enabled && self.hover(r);
        // Quiet "ghost" buttons: a faint wash, an outline only on hover,
        // and a soft glow when active.
        let base = if enabled { color } else { pal::FAINT };
        let fill = if active {
            with_alpha(base, 0.20)
        } else if hover {
            with_alpha(base, 0.12)
        } else {
            with_alpha(base, 0.045)
        };
        draw_rectangle(r.x, r.y, r.w, r.h, fill);
        if hover || active {
            draw_rectangle_lines(r.x, r.y, r.w, r.h, 1.0, with_alpha(base, if active { 0.55 } else { 0.4 }));
        }
        if active {
            draw_rectangle(r.x + 6.0, r.y + r.h - 2.0, r.w - 12.0, 2.0, with_alpha(base, 0.8));
        }
        let mut size = (r.h * 0.46).clamp(11.0, 15.0);
        let tw = gfx::text_width(label, size);
        if tw > r.w - 12.0 && tw > 0.0 {
            size = (size * (r.w - 12.0) / tw).max(9.0);
        }
        let tc = if !enabled {
            pal::FAINT
        } else if active || hover {
            mix(base, WHITE, 0.6)
        } else {
            with_alpha(mix(base, WHITE, 0.45), 0.8)
        };
        gfx::text_centered(label, r.x + r.w / 2.0, r.y + (r.h - size) / 2.0, size, tc);
        enabled && self.click(r)
    }

    /// Draw pending tooltip; call last.
    pub fn end_frame(&mut self) {
        self.sync_keyboard();
        if let Some(t) = self.tooltip.take() {
            let (sw, sh) = (screen_width(), screen_height());
            let size = 14.0;
            let lines = gfx::wrap(&t, size, (sw - 24.0).min(560.0));
            let w = lines.iter().map(|l| gfx::text_width(l, size)).fold(0.0, f32::max) + 16.0;
            let h = lines.len() as f32 * (size + 4.0) + 8.0;
            // Beside the cursor, or above the finger that would hide it.
            let mut p = if self.touch { self.mouse - vec2(w / 2.0, h + 44.0) } else { self.mouse + vec2(14.0, 18.0) };
            p.x = p.x.min(sw - w - 4.0).max(4.0);
            if p.y + h > sh {
                p.y = self.mouse.y - h - 10.0;
            }
            p.y = p.y.max(4.0);
            let r = Rect::new(p.x, p.y, w, h);
            panel(r, pal::ACCENT);
            for (i, l) in lines.iter().enumerate() {
                gfx::text(l, r.x + 8.0, r.y + 6.0 + i as f32 * (size + 4.0), size, pal::TEXT);
            }
        }
    }
}

/// A calm translucent pane with a hairline edge and a thin accent on top.
pub fn panel(r: Rect, edge: Color) {
    draw_rectangle(r.x, r.y, r.w, r.h, pal::PANEL);
    draw_rectangle_lines(r.x, r.y, r.w, r.h, 1.0, with_alpha(edge, edge.a * 0.45));
    draw_rectangle(r.x + 1.0, r.y, (r.w * 0.25).min(60.0), 1.5, with_alpha(edge, 0.8));
}

/// Single-line text input.
pub struct TextField {
    pub text: String,
    pub focused: bool,
    pub max: usize,
    pub filter: fn(char) -> bool,
}

impl TextField {
    pub fn new(text: &str, max: usize, filter: fn(char) -> bool) -> TextField {
        TextField { text: text.to_string(), focused: false, max, filter }
    }

    /// Returns true when Enter is pressed.
    pub fn ui(&mut self, ui: &mut Ui, r: Rect, placeholder: &str) -> bool {
        ui.block(r);
        if ui.pressed {
            let inside = r.contains(ui.mouse);
            if inside {
                ui.pressed = false;
                ui.text_touched = true;
            }
            self.focused = inside;
        }
        let mut enter = false;
        if self.focused {
            ui.keyboard_taken = true;
            ui.text_focus = true;
            for t in ui.typed.clone() {
                match t {
                    Typed::Char(c) => {
                        if !c.is_control() && (self.filter)(c) && self.text.chars().count() < self.max {
                            self.text.push(c);
                        }
                    }
                    Typed::Back => {
                        self.text.pop();
                    }
                    Typed::Enter => enter = true,
                }
            }
            for &c in &ui.chars {
                if !c.is_control() && (self.filter)(c) && self.text.chars().count() < self.max {
                    self.text.push(c);
                }
            }
            if ui.rep(KeyCode::Backspace) {
                self.text.pop();
            }
            if ui.ctrl && is_key_pressed(KeyCode::V)
                && let Some(s) = macroquad::miniquad::window::clipboard_get() {
                    for c in s.chars() {
                        if !c.is_control() && (self.filter)(c) && self.text.chars().count() < self.max {
                            self.text.push(c);
                        }
                    }
                }
            if is_key_pressed(KeyCode::Enter) || is_key_pressed(KeyCode::KpEnter) {
                enter = true;
            }
            if is_key_pressed(KeyCode::Escape) {
                self.focused = false;
            }
        }
        let edge = if self.focused { pal::ACCENT } else { pal::FAINT };
        draw_rectangle(r.x, r.y, r.w, r.h, with_alpha(BLACK, 0.5));
        draw_rectangle_lines(r.x, r.y, r.w, r.h, 1.5, edge);
        let size = (r.h * 0.55).clamp(12.0, 20.0);
        let ty = r.y + (r.h - size) / 2.0;
        if self.text.is_empty() && !self.focused {
            gfx::text(placeholder, r.x + 8.0, ty, size, pal::DIM);
        } else {
            gfx::text(&self.text, r.x + 8.0, ty, size, pal::TEXT);
        }
        if self.focused && (ui.time * 2.0).fract() < 0.6 {
            let cx = r.x + 8.0 + gfx::text_width(&self.text, size) + 1.0;
            draw_line(cx, ty, cx, ty + size, 2.0, pal::ACCENT);
        }
        enter
    }
}
