//! Purely cosmetic particles and beams, in world (tile) coordinates.

use macroquad::prelude::*;
use macroquad::rand::gen_range;

use crate::gfx::{arc, with_alpha};
use crate::view::Cam;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Dot,
    Spark,
    Ring,
}

pub struct Particle {
    pos: Vec2,
    vel: Vec2,
    life: f32,
    max: f32,
    color: Color,
    size: f32,
    shape: Shape,
}

pub struct Beam {
    a: Vec2,
    b: Vec2,
    color: Color,
    width: f32,
    life: f32,
    max: f32,
}

const MAX_PARTICLES: usize = 4000;

#[derive(Default)]
pub struct Fx {
    parts: Vec<Particle>,
    beams: Vec<Beam>,
}

impl Fx {
    pub fn update(&mut self, dt: f32) {
        for p in &mut self.parts {
            p.life -= dt;
            p.pos += p.vel * dt;
            p.vel *= (1.0 - 3.0 * dt).max(0.0);
        }
        self.parts.retain(|p| p.life > 0.0);
        for b in &mut self.beams {
            b.life -= dt;
        }
        self.beams.retain(|b| b.life > 0.0);
    }

    fn push(&mut self, p: Particle) {
        if self.parts.len() < MAX_PARTICLES {
            self.parts.push(p);
        }
    }

    pub fn burst(&mut self, at: Vec2, color: Color, n: usize, speed: f32, life: f32) {
        for _ in 0..n {
            let a = gen_range(0.0, std::f32::consts::TAU);
            let s = speed * gen_range(0.3, 1.0);
            self.push(Particle {
                pos: at,
                vel: Vec2::from_angle(a) * s,
                life: life * gen_range(0.6, 1.0),
                max: life,
                color,
                size: gen_range(0.04, 0.09),
                shape: Shape::Spark,
            });
        }
    }

    pub fn dots(&mut self, at: Vec2, color: Color, n: usize, speed: f32, life: f32) {
        for _ in 0..n {
            let a = gen_range(0.0, std::f32::consts::TAU);
            self.push(Particle {
                pos: at,
                vel: Vec2::from_angle(a) * speed * gen_range(0.2, 1.0),
                life: life * gen_range(0.5, 1.0),
                max: life,
                color,
                size: gen_range(0.03, 0.06),
                shape: Shape::Dot,
            });
        }
    }

    pub fn ring(&mut self, at: Vec2, color: Color, radius: f32, life: f32) {
        self.push(Particle { pos: at, vel: Vec2::ZERO, life, max: life, color, size: radius, shape: Shape::Ring });
    }

    pub fn trail(&mut self, at: Vec2, color: Color) {
        if self.parts.len() < MAX_PARTICLES / 2 {
            self.push(Particle {
                pos: at,
                vel: Vec2::ZERO,
                life: 0.45,
                max: 0.45,
                color,
                size: 0.05,
                shape: Shape::Dot,
            });
        }
    }

    pub fn beam(&mut self, a: Vec2, b: Vec2, color: Color, width: f32, life: f32) {
        if self.beams.len() < 2000 {
            self.beams.push(Beam { a, b, color, width, life, max: life });
        }
    }

    pub fn draw(&self, cam: &Cam) {
        let z = cam.zoom;
        for b in &self.beams {
            let t = (b.life / b.max).clamp(0.0, 1.0);
            let (a, e) = (cam.w2s(b.a), cam.w2s(b.b));
            draw_line(a.x, a.y, e.x, e.y, (b.width * z * t).max(1.0), with_alpha(b.color, t));
            draw_circle(e.x, e.y, b.width * z * 1.2 * t, with_alpha(b.color, t));
        }
        for p in &self.parts {
            let t = (p.life / p.max).clamp(0.0, 1.0);
            let s = cam.w2s(p.pos);
            match p.shape {
                Shape::Dot => draw_circle(s.x, s.y, (p.size * z * t).max(0.6), with_alpha(p.color, t)),
                Shape::Spark => {
                    let tail = s - p.vel * z * 0.05;
                    draw_line(s.x, s.y, tail.x, tail.y, (p.size * z * 0.6).max(1.0), with_alpha(p.color, t));
                }
                Shape::Ring => {
                    let r = p.size * z * (1.0 - t * t).max(0.05);
                    arc(s, r, 0.0, std::f32::consts::TAU, (0.08 * z * t).max(1.0), with_alpha(p.color, t * 0.9));
                }
            }
        }
    }
}
