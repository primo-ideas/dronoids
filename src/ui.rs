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
}

impl Ui {
    pub fn begin_frame(&mut self) {
        let dt = get_frame_time();
        self.time += dt;
        self.mouse = mouse_position().into();
        self.pressed = is_mouse_button_pressed(MouseButton::Left);
        self.down = is_mouse_button_down(MouseButton::Left);
        self.released = is_mouse_button_released(MouseButton::Left);
        self.right_pressed = is_mouse_button_pressed(MouseButton::Right);
        self.wheel = mouse_wheel().1;
        self.double_click = false;
        if self.pressed {
            self.double_click = self.time - self.last_click < 0.3;
            self.last_click = self.time;
        }
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
        if let Some(t) = self.tooltip.take() {
            let size = 14.0;
            let w = gfx::text_width(&t, size) + 16.0;
            let mut p = self.mouse + vec2(14.0, 18.0);
            if p.x + w > screen_width() {
                p.x = screen_width() - w - 4.0;
            }
            let r = Rect::new(p.x, p.y, w, size + 12.0);
            panel(r, pal::ACCENT);
            gfx::text(&t, r.x + 8.0, r.y + 6.0, size, pal::TEXT);
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
            }
            self.focused = inside;
        }
        let mut enter = false;
        if self.focused {
            ui.keyboard_taken = true;
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
