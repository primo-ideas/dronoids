//! Palette, text helpers and the bloom post-process that gives everything
//! its neon halo.

use std::cell::RefCell;

use macroquad::miniquad::{BlendFactor, BlendState, BlendValue, Equation};
use macroquad::prelude::*;

pub mod pal {
    use macroquad::prelude::Color;

    pub const BG: Color = Color::new(0.012, 0.016, 0.04, 1.0);
    pub const WALL_FILL: Color = Color::new(0.06, 0.035, 0.14, 1.0);
    pub const WALL_EDGE: Color = Color::from_hex(0x8a4dff);
    pub const ORE: Color = Color::from_hex(0xffc23a);
    pub const ORE_CORE: Color = Color::from_hex(0xfff2c0);
    pub const TEXT: Color = Color::from_hex(0xc9d6f2);
    pub const DIM: Color = Color::from_hex(0x66739c);
    pub const FAINT: Color = Color::from_hex(0x2c3558);
    pub const PANEL: Color = Color::new(0.016, 0.024, 0.06, 0.78);
    pub const PANEL_EDGE: Color = Color::new(0.35, 0.6, 1.0, 0.6);
    pub const ACCENT: Color = Color::from_hex(0x00e5ff);
    pub const GOOD: Color = Color::from_hex(0x5cff8a);
    pub const BAD: Color = Color::from_hex(0xff3860);
    pub const WARN: Color = Color::from_hex(0xffb020);
    pub const VIOLET: Color = Color::from_hex(0xb07cff);
    pub const PINK: Color = Color::from_hex(0xff5fd2);
}

/// Neon colors for up to 16 players (none of them amber, the ore color).
const PLAYER_COLORS: [u32; 16] = [
    0x00e5ff, 0xff2bd6, 0x7dff3a, 0xff8a1f, 0xa86bff, 0xff3b5c, 0x2bffc6, 0x4d9dff, 0xff7eb6, 0xc6ff3a, 0xff6a4d,
    0xd0a0ff, 0x9dffd8, 0x5b6bff, 0xff4d9d, 0xe0f0ff,
];

pub fn player_color(id: u8) -> Color {
    Color::from_hex(PLAYER_COLORS[id as usize % PLAYER_COLORS.len()])
}

/// One colour per kind of bonus, on the relics and wherever a factory
/// shows what it has.
pub fn bonus_color(b: dronoids::rules::Bonus) -> Color {
    use dronoids::rules::Bonus;
    Color::from_hex(match b {
        Bonus::Forge => 0xff8a1f,
        Bonus::Refinery => 0x5cff8a,
        Bonus::Bulwark => 0x4d9dff,
        Bonus::Arsenal => 0xff3860,
    })
}

pub fn with_alpha(c: Color, a: f32) -> Color {
    Color::new(c.r, c.g, c.b, a)
}

pub fn scale_rgb(c: Color, k: f32) -> Color {
    Color::new(c.r * k, c.g * k, c.b * k, c.a)
}

pub fn mix(a: Color, b: Color, t: f32) -> Color {
    Color::new(
        a.r + (b.r - a.r) * t,
        a.g + (b.g - a.g) * t,
        a.b + (b.b - a.b) * t,
        a.a + (b.a - a.a) * t,
    )
}

// ---------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------

thread_local! {
    static FONT: RefCell<Option<Font>> = const { RefCell::new(None) };
}

/// Fira Mono (SIL Open Font License, see assets/fonts), embedded so the
/// desktop and the browser look the same.
pub fn init_font() {
    if let Ok(mut font) = load_ttf_font_from_bytes(include_bytes!("../assets/fonts/FiraMono-Medium.ttf")) {
        font.set_filter(FilterMode::Linear);
        FONT.with(|f| *f.borrow_mut() = Some(font));
    }
}

/// Draw text with its top-left corner at (x, y).
pub fn text(s: &str, x: f32, y: f32, size: f32, color: Color) {
    FONT.with(|f| {
        let f = f.borrow();
        draw_text_ex(
            s,
            x.round(),
            (y + size * 0.78).round(),
            TextParams { font: f.as_ref(), font_size: size as u16, color, ..Default::default() },
        );
    });
}

pub fn text_width(s: &str, size: f32) -> f32 {
    FONT.with(|f| measure_text(s, f.borrow().as_ref(), size as u16, 1.0).width)
}

pub fn text_centered(s: &str, cx: f32, y: f32, size: f32, color: Color) {
    text(s, cx - text_width(s, size) / 2.0, y, size, color);
}

/// Width of one character of the (monospace) font.
pub fn char_width(size: f32) -> f32 {
    text_width("MMMMMMMMMM", size) / 10.0
}

/// `s` cut at spaces into lines no wider than `w` (a longer word stays whole).
pub fn wrap(s: &str, size: f32, w: f32) -> Vec<String> {
    let mut lines = vec![];
    let mut line = String::new();
    for word in s.split(' ') {
        let longer = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
        if !line.is_empty() && text_width(&longer, size) > w {
            lines.push(std::mem::replace(&mut line, word.to_string()));
        } else {
            line = longer;
        }
    }
    lines.push(line);
    lines
}

/// The largest size up to `size` at which `s` fits in `w` (never below 9).
pub fn fit_size(s: &str, size: f32, w: f32) -> f32 {
    let tw = text_width(s, size);
    if tw > w && tw > 0.0 { (size * w / tw).floor().max(9.0) } else { size }
}

/// Coloured pieces of text, cut at spaces into lines no wider than `w`,
/// each centred on `cx`, the first at `y`. Returns how many lines.
pub fn rich_wrapped(parts: &[(String, Color)], size: f32, cx: f32, y: f32, w: f32, backdrop: Color) -> usize {
    let mut lines: Vec<Vec<(String, Color)>> = vec![vec![]];
    let mut used = 0.0;
    for (t, c) in parts {
        for word in t.split_inclusive(' ') {
            let ww = text_width(word, size);
            let trimmed = text_width(word.trim_end(), size);
            if used > 0.0 && used + trimmed > w {
                lines.push(vec![]);
                used = 0.0;
            }
            // A line starts with words, not with blanks or a separator.
            if used == 0.0 && matches!(word.trim(), "" | "·") {
                continue;
            }
            lines.last_mut().unwrap().push((word.to_string(), *c));
            used += ww;
        }
    }
    let lh = size + 12.0;
    for (i, line) in lines.iter().enumerate() {
        let full: String = line.iter().map(|p| p.0.as_str()).collect();
        let lw = text_width(full.trim_end(), size);
        let mut x = cx - lw / 2.0;
        let ly = y + i as f32 * lh;
        draw_rectangle(x - 14.0, ly - 5.0, lw + 28.0, lh, backdrop);
        for (t, c) in line {
            text(t, x, ly, size, *c);
            x += text_width(t, size);
        }
    }
    lines.len()
}

// ---------------------------------------------------------------------------
// Shapes
// ---------------------------------------------------------------------------

/// Arc of a circle as line segments, angles in radians.
pub fn arc(c: Vec2, r: f32, from: f32, to: f32, thick: f32, color: Color) {
    let span = to - from;
    let n = ((span.abs() * r / 4.0).ceil() as usize).clamp(3, 96);
    let mut prev = c + Vec2::from_angle(from) * r;
    for i in 1..=n {
        let a = from + span * i as f32 / n as f32;
        let p = c + Vec2::from_angle(a) * r;
        draw_line(prev.x, prev.y, p.x, p.y, thick, color);
        prev = p;
    }
}

/// Closed polygon outline.
pub fn poly_lines(pts: &[Vec2], thick: f32, color: Color) {
    for i in 0..pts.len() {
        let a = pts[i];
        let b = pts[(i + 1) % pts.len()];
        draw_line(a.x, a.y, b.x, b.y, thick, color);
    }
}

/// Convex polygon fill (triangle fan).
pub fn poly_fill(pts: &[Vec2], color: Color) {
    for i in 1..pts.len().saturating_sub(1) {
        draw_triangle(pts[0], pts[i], pts[i + 1], color);
    }
}

pub fn regular(c: Vec2, r: f32, sides: usize, rot: f32) -> Vec<Vec2> {
    (0..sides)
        .map(|i| c + Vec2::from_angle(rot + i as f32 * std::f32::consts::TAU / sides as f32) * r)
        .collect()
}

// ---------------------------------------------------------------------------
// Bloom
// ---------------------------------------------------------------------------

const VERTEX: &str = r#"#version 100
precision highp float;
attribute vec3 position;
attribute vec2 texcoord;
attribute vec4 color0;
varying vec2 uv;
varying vec4 color;
uniform mat4 Model;
uniform mat4 Projection;
void main() {
    gl_Position = Projection * Model * vec4(position, 1.0);
    color = color0 / 255.0;
    uv = texcoord;
}
"#;

const BRIGHT: &str = r#"#version 100
precision highp float;
varying vec2 uv;
varying vec4 color;
uniform sampler2D Texture;
uniform float u_threshold;
void main() {
    vec3 c = texture2D(Texture, uv).rgb;
    float l = max(c.r, max(c.g, c.b));
    float k = smoothstep(u_threshold, u_threshold + 0.35, l);
    gl_FragColor = vec4(c * k, 1.0);
}
"#;

const BLUR: &str = r#"#version 100
precision highp float;
varying vec2 uv;
varying vec4 color;
uniform sampler2D Texture;
uniform vec2 u_dir;
void main() {
    vec3 c = texture2D(Texture, uv).rgb * 0.2270270270;
    c += texture2D(Texture, uv + u_dir * 1.3846153846).rgb * 0.3162162162;
    c += texture2D(Texture, uv - u_dir * 1.3846153846).rgb * 0.3162162162;
    c += texture2D(Texture, uv + u_dir * 3.2307692308).rgb * 0.0702702703;
    c += texture2D(Texture, uv - u_dir * 3.2307692308).rgb * 0.0702702703;
    gl_FragColor = vec4(c, 1.0);
}
"#;

const ADD: &str = r#"#version 100
precision highp float;
varying vec2 uv;
varying vec4 color;
uniform sampler2D Texture;
uniform float u_intensity;
void main() {
    gl_FragColor = vec4(texture2D(Texture, uv).rgb * u_intensity, 1.0);
}
"#;

const COPY: &str = r#"#version 100
precision highp float;
varying vec2 uv;
varying vec4 color;
uniform sampler2D Texture;
uniform vec2 u_screen;
void main() {
    vec3 c = texture2D(Texture, uv).rgb;
    // Soft vignette.
    vec2 d = uv - 0.5;
    float v = 1.0 - dot(d, d) * 0.55;
    gl_FragColor = vec4(c * v, 1.0);
}
"#;

struct Level {
    a: RenderTarget,
    b: RenderTarget,
    w: f32,
    h: f32,
}

pub struct Bloom {
    size: (u32, u32),
    pub scene: RenderTarget,
    levels: Vec<Level>,
    bright: Material,
    blur: Material,
    add: Material,
    copy: Material,
    pub intensity: f32,
}

fn make_rt(w: u32, h: u32) -> RenderTarget {
    let rt = render_target(w.max(1), h.max(1));
    rt.texture.set_filter(FilterMode::Linear);
    rt
}

fn material(fragment: &str, uniforms: Vec<UniformDesc>, additive: bool) -> Material {
    let pipeline_params = if additive {
        PipelineParams {
            color_blend: Some(BlendState::new(Equation::Add, BlendFactor::One, BlendFactor::One)),
            ..Default::default()
        }
    } else {
        PipelineParams::default()
    };
    load_material(
        ShaderSource::Glsl { vertex: VERTEX, fragment },
        MaterialParams { pipeline_params, uniforms, textures: vec![] },
    )
    .expect("shader compiles")
}

fn rt_camera(rt: &RenderTarget, w: f32, h: f32) -> Camera2D {
    let mut cam = Camera2D::from_display_rect(Rect::new(0.0, 0.0, w, h));
    cam.render_target = Some(rt.clone());
    cam
}

fn blit(src: &Texture2D, w: f32, h: f32, color: Color) {
    draw_texture_ex(
        src,
        0.0,
        0.0,
        color,
        DrawTextureParams { dest_size: Some(vec2(w, h)), flip_y: true, ..Default::default() },
    );
}

impl Bloom {
    pub fn new() -> Bloom {
        let bright = material(BRIGHT, vec![UniformDesc::new("u_threshold", UniformType::Float1)], false);
        let blur = material(BLUR, vec![UniformDesc::new("u_dir", UniformType::Float2)], false);
        let add = material(ADD, vec![UniformDesc::new("u_intensity", UniformType::Float1)], true);
        let copy = material(COPY, vec![UniformDesc::new("u_screen", UniformType::Float2)], false);
        let mut b = Bloom {
            size: (0, 0),
            scene: make_rt(1, 1),
            levels: vec![],
            bright,
            blur,
            add,
            copy,
            intensity: 1.0,
        };
        b.resize();
        b
    }

    fn resize(&mut self) {
        let (w, h) = (screen_width().max(1.0) as u32, screen_height().max(1.0) as u32);
        if self.size == (w, h) {
            return;
        }
        self.size = (w, h);
        self.scene = make_rt(w, h);
        self.levels = [2u32, 4, 8, 16]
            .iter()
            .map(|d| {
                let (lw, lh) = ((w / d).max(1), (h / d).max(1));
                Level { a: make_rt(lw, lh), b: make_rt(lw, lh), w: lw as f32, h: lh as f32 }
            })
            .collect();
    }

    /// Start drawing the glowing part of the frame (screen coordinates).
    pub fn begin(&mut self) {
        self.resize();
        let (w, h) = (self.size.0 as f32, self.size.1 as f32);
        set_camera(&rt_camera(&self.scene, w, h));
        clear_background(pal::BG);
    }

    /// Blur, then composite scene + halos onto the screen.
    pub fn finish(&self) {
        let (w, h) = (self.size.0 as f32, self.size.1 as f32);

        // Bright pass into the first level, then successive downsamples.
        for (i, lv) in self.levels.iter().enumerate() {
            set_camera(&rt_camera(&lv.a, lv.w, lv.h));
            clear_background(BLACK);
            if i == 0 {
                self.bright.set_uniform("u_threshold", 0.12f32);
                gl_use_material(&self.bright);
                blit(&self.scene.texture, lv.w, lv.h, WHITE);
            } else {
                blit(&self.levels[i - 1].a.texture, lv.w, lv.h, WHITE);
            }
            gl_use_default_material();
        }
        // Separable blur on every level.
        for lv in &self.levels {
            for (src, dst, dir) in [(&lv.a, &lv.b, vec2(1.0 / lv.w, 0.0)), (&lv.b, &lv.a, vec2(0.0, 1.0 / lv.h))] {
                set_camera(&rt_camera(dst, lv.w, lv.h));
                clear_background(BLACK);
                self.blur.set_uniform("u_dir", dir);
                gl_use_material(&self.blur);
                blit(&src.texture, lv.w, lv.h, WHITE);
                gl_use_default_material();
            }
        }

        set_default_camera();
        clear_background(BLACK);
        self.copy.set_uniform("u_screen", vec2(w, h));
        gl_use_material(&self.copy);
        blit(&self.scene.texture, w, h, WHITE);
        gl_use_material(&self.add);
        let weights = [0.5, 0.58, 0.66, 0.75];
        for (lv, k) in self.levels.iter().zip(weights) {
            self.add.set_uniform("u_intensity", k * self.intensity);
            blit(&lv.a.texture, w, h, WHITE);
        }
        gl_use_default_material();
    }
}

// ---------------------------------------------------------------------------
// Fog of war
// ---------------------------------------------------------------------------

const MULTIPLY: &str = r#"#version 100
precision highp float;
varying vec2 uv;
varying vec4 color;
uniform sampler2D Texture;
void main() {
    gl_FragColor = vec4(texture2D(Texture, uv).rgb, 1.0);
}
"#;

/// The mask is painted at a fraction of the screen resolution, blurred
/// (soft edges for free), then multiplied over the scene before the bloom,
/// so that hidden space neither shows nor glows.
pub struct Fog {
    size: (u32, u32),
    a: RenderTarget,
    b: RenderTarget,
    blur: Material,
    multiply: Material,
}

const FOG_DIV: u32 = 4;

impl Fog {
    pub fn new() -> Fog {
        let blur = material(BLUR, vec![UniformDesc::new("u_dir", UniformType::Float2)], false);
        let multiply = load_material(
            ShaderSource::Glsl { vertex: VERTEX, fragment: MULTIPLY },
            MaterialParams {
                pipeline_params: PipelineParams {
                    color_blend: Some(BlendState::new(
                        Equation::Add,
                        BlendFactor::Value(BlendValue::DestinationColor),
                        BlendFactor::Zero,
                    )),
                    ..Default::default()
                },
                uniforms: vec![],
                textures: vec![],
            },
        )
        .expect("shader compiles");
        Fog { size: (0, 0), a: make_rt(1, 1), b: make_rt(1, 1), blur, multiply }
    }

    fn resize(&mut self) {
        let w = (screen_width().max(1.0) as u32 / FOG_DIV).max(1);
        let h = (screen_height().max(1.0) as u32 / FOG_DIV).max(1);
        if self.size != (w, h) {
            self.size = (w, h);
            self.a = make_rt(w, h);
            self.b = make_rt(w, h);
        }
    }

    /// `paint` draws the mask in screen coordinates over `unknown`: white
    /// where we see, darker where we do not.
    pub fn apply(&mut self, bloom: &Bloom, unknown: Color, paint: impl FnOnce()) {
        self.resize();
        let (mw, mh) = (self.size.0 as f32, self.size.1 as f32);
        let mut cam = Camera2D::from_display_rect(Rect::new(0.0, 0.0, screen_width(), screen_height()));
        cam.render_target = Some(self.a.clone());
        set_camera(&cam);
        clear_background(unknown);
        paint();
        for _ in 0..2 {
            for (src, dst, dir) in [(&self.a, &self.b, vec2(1.0 / mw, 0.0)), (&self.b, &self.a, vec2(0.0, 1.0 / mh))] {
                set_camera(&rt_camera(dst, mw, mh));
                clear_background(BLACK);
                self.blur.set_uniform("u_dir", dir);
                gl_use_material(&self.blur);
                blit(&src.texture, mw, mh, WHITE);
                gl_use_default_material();
            }
        }
        let (w, h) = (bloom.size.0 as f32, bloom.size.1 as f32);
        set_camera(&rt_camera(&bloom.scene, w, h));
        gl_use_material(&self.multiply);
        blit(&self.a.texture, w, h, WHITE);
        gl_use_default_material();
    }
}
