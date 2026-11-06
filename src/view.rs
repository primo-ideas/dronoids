//! Draws the world: grid, walls, ore, factories, drones and effects.

use std::collections::HashMap;
use std::f32::consts::{PI, TAU};

use macroquad::prelude::*;

use game_of_drones::rules::{FACTORY_HP, ORE_MAX, SPAWN_INTERVAL, TICKS_PER_SEC};
use game_of_drones::sim::{Event, LastAction, Tile, World};

use crate::fx::Fx;
use crate::gfx::{self, arc, mix, pal, player_color, poly_fill, poly_lines, regular, scale_rgb, with_alpha};

#[derive(Clone, Copy, Debug)]
pub struct Cam {
    pub center: Vec2,
    /// Pixels per tile.
    pub zoom: f32,
    pub shake: Vec2,
}

impl Cam {
    pub fn w2s(&self, p: Vec2) -> Vec2 {
        (p - self.center - self.shake) * self.zoom + vec2(screen_width(), screen_height()) * 0.5
    }

    pub fn s2w(&self, s: Vec2) -> Vec2 {
        (s - vec2(screen_width(), screen_height()) * 0.5) / self.zoom + self.center + self.shake
    }

    pub fn tile_at(&self, s: Vec2) -> (i32, i32) {
        let w = self.s2w(s);
        (w.x.floor() as i32, w.y.floor() as i32)
    }
}

pub fn tile_center(x: i32, y: i32) -> Vec2 {
    vec2(x as f32 + 0.5, y as f32 + 0.5)
}

fn hash2(x: i32, y: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(374_761_393) ^ (y as u32).wrapping_mul(668_265_263);
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    (h & 0xffff) as f32 / 65535.0
}

fn facing_angle(f: u8) -> f32 {
    match f % 4 {
        0 => -PI / 2.0,
        1 => 0.0,
        2 => PI / 2.0,
        _ => PI,
    }
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub struct Highlight {
    pub factory: Option<u32>,
    pub drone: Option<u32>,
    /// Build ghost: tile and validity.
    pub ghost: Option<((i32, i32), bool)>,
    pub build_range_for: Option<u8>,
}

#[derive(Default)]
pub struct WorldView {
    pub fx: Fx,
    angles: HashMap<u32, f32>,
    pub time: f32,
    pub shake: f32,
}

impl WorldView {
    /// Turn simulation events into eye candy.
    pub fn on_events(&mut self, world: &World, speed: f32) {
        let tick_dt = 1.0 / (TICKS_PER_SEC * speed.max(1.0));
        let heavy = speed > 4.0;
        for e in &world.events {
            match *e {
                Event::Spawn { owner, x, y, .. } => {
                    self.fx.ring(tile_center(x, y), player_color(owner), 0.7, 0.35);
                }
                Event::Mine { x, y, tx, ty, depleted, .. } => {
                    self.fx.beam(tile_center(x, y), tile_center(tx, ty), pal::ORE, 0.05, tick_dt * 0.9);
                    if !heavy {
                        self.fx.dots(tile_center(tx, ty), pal::ORE, 2, 1.5, 0.35);
                    }
                    if depleted {
                        self.fx.burst(tile_center(tx, ty), pal::ORE, 12, 4.0, 0.6);
                    }
                }
                Event::Drop { x, y, tx, ty, .. } => {
                    self.fx.beam(tile_center(x, y), tile_center(tx, ty), pal::ORE_CORE, 0.07, tick_dt * 1.5);
                    self.fx.ring(tile_center(tx, ty), pal::ORE, 0.5, 0.3);
                }
                Event::Attack { owner, x, y, tx, ty } => {
                    let c = mix(player_color(owner), WHITE, 0.35);
                    self.fx.beam(tile_center(x, y), tile_center(tx, ty), c, 0.08, tick_dt);
                    if !heavy {
                        self.fx.burst(tile_center(tx, ty), c, 3, 3.0, 0.25);
                    }
                }
                Event::DroneDestroyed { owner, x, y } => {
                    let at = tile_center(x, y);
                    self.fx.burst(at, player_color(owner), 16, 6.0, 0.7);
                    self.fx.ring(at, player_color(owner), 1.1, 0.45);
                }
                Event::FactoryDestroyed { owner, x, y } => {
                    let at = tile_center(x, y);
                    self.fx.burst(at, player_color(owner), 60, 9.0, 1.2);
                    self.fx.burst(at, WHITE, 20, 6.0, 0.8);
                    self.fx.ring(at, player_color(owner), 3.5, 1.0);
                    self.fx.ring(at, WHITE, 2.0, 0.6);
                    self.shake = self.shake.max(0.35);
                }
                Event::FactoryBuilt { owner, x, y } => {
                    self.fx.ring(tile_center(x, y), player_color(owner), 2.5, 0.8);
                    self.fx.dots(tile_center(x, y), player_color(owner), 20, 4.0, 0.8);
                }
                Event::PlayerEliminated { .. } => {
                    self.shake = self.shake.max(0.5);
                }
            }
        }
        if speed <= 2.0 {
            for d in &world.drones {
                if (d.x, d.y) != (d.prev_x, d.prev_y) && d.born != world.tick.wrapping_sub(1) {
                    self.fx.trail(tile_center(d.prev_x, d.prev_y), with_alpha(player_color(d.owner), 0.5));
                }
            }
        }
    }

    pub fn update(&mut self, dt: f32, world: &World) {
        self.time += dt;
        self.fx.update(dt);
        self.shake = (self.shake - dt).max(0.0);
        let k = (dt * 14.0).min(1.0);
        for d in &world.drones {
            let target = facing_angle(d.facing);
            let a = self.angles.entry(d.id).or_insert(target);
            let mut diff = (target - *a) % TAU;
            if diff > PI {
                diff -= TAU;
            } else if diff < -PI {
                diff += TAU;
            }
            *a += diff * k;
        }
        if self.angles.len() > world.drones.len() * 2 + 64 {
            let alive: std::collections::HashSet<u32> = world.drones.iter().map(|d| d.id).collect();
            self.angles.retain(|id, _| alive.contains(id));
        }
    }

    pub fn shake_offset(&self) -> Vec2 {
        if self.shake <= 0.0 {
            return Vec2::ZERO;
        }
        let s = self.shake * 0.6;
        vec2((self.time * 71.0).sin() * s, (self.time * 53.0).cos() * s)
    }

    /// Screen position of a drone, interpolated between ticks.
    pub fn drone_pos(world: &World, id: u32, alpha: f32) -> Option<Vec2> {
        let d = world.drone(id)?;
        let a = smooth(alpha);
        let p = vec2(d.prev_x as f32, d.prev_y as f32).lerp(vec2(d.x as f32, d.y as f32), a);
        Some(p + vec2(0.5, 0.5))
    }

    pub fn draw(&self, world: &World, cam: &Cam, alpha: f32, hl: &Highlight) {
        let z = cam.zoom;
        let tl = cam.s2w(vec2(0.0, 0.0));
        let br = cam.s2w(vec2(screen_width(), screen_height()));
        let x0 = (tl.x.floor() as i32 - 1).max(0);
        let y0 = (tl.y.floor() as i32 - 1).max(0);
        let x1 = (br.x.ceil() as i32 + 1).min(world.w);
        let y1 = (br.y.ceil() as i32 + 1).min(world.h);
        let t = self.time;

        // Grid.
        if z > 9.0 {
            let a = cam.w2s(vec2(x0 as f32, y0 as f32));
            let b = cam.w2s(vec2(x1 as f32, y1 as f32));
            for x in x0..=x1 {
                let sx = cam.w2s(vec2(x as f32, 0.0)).x;
                draw_line(sx, a.y, sx, b.y, 1.0, pal::GRID);
            }
            for y in y0..=y1 {
                let sy = cam.w2s(vec2(0.0, y as f32)).y;
                draw_line(a.x, sy, b.x, sy, 1.0, pal::GRID);
            }
        }

        // Build range hint.
        if let Some(owner) = hl.build_range_for {
            self.draw_build_range(world, cam, owner, (x0, y0, x1, y1));
        }

        // Outside the map: solid rock.
        let (sw, sh) = (screen_width(), screen_height());
        let m0 = cam.w2s(vec2(0.0, 0.0));
        let m1 = cam.w2s(vec2(world.w as f32, world.h as f32));
        let rock = scale_rgb(pal::WALL_FILL, 0.75);
        if m0.x > 0.0 {
            draw_rectangle(0.0, 0.0, m0.x, sh, rock);
        }
        if m1.x < sw {
            draw_rectangle(m1.x, 0.0, sw - m1.x, sh, rock);
        }
        if m0.y > 0.0 {
            draw_rectangle(m0.x.max(0.0), 0.0, m1.x.min(sw) - m0.x.max(0.0), m0.y, rock);
        }
        if m1.y < sh {
            draw_rectangle(m0.x.max(0.0), m1.y, m1.x.min(sw) - m0.x.max(0.0), sh - m1.y, rock);
        }

        // Walls: dark fill, neon edges where they meet open ground.
        let edge = (z * 0.075).max(1.5);
        for y in y0..y1 {
            for x in x0..x1 {
                if world.tile(x, y) != Tile::Wall {
                    continue;
                }
                let p = cam.w2s(vec2(x as f32, y as f32));
                let shade = 0.8 + 0.35 * hash2(x, y);
                draw_rectangle(p.x, p.y, z + 0.5, z + 0.5, scale_rgb(pal::WALL_FILL, shade));
            }
        }
        for y in y0..y1 {
            for x in x0..x1 {
                if world.tile(x, y) != Tile::Wall {
                    continue;
                }
                let p = cam.w2s(vec2(x as f32, y as f32));
                let open = |dx: i32, dy: i32| world.inside(x + dx, y + dy) && world.tile(x + dx, y + dy) != Tile::Wall;
                let c = pal::WALL_EDGE;
                if open(0, -1) {
                    draw_line(p.x, p.y, p.x + z, p.y, edge, c);
                }
                if open(0, 1) {
                    draw_line(p.x, p.y + z, p.x + z, p.y + z, edge, c);
                }
                if open(-1, 0) {
                    draw_line(p.x, p.y, p.x, p.y + z, edge, c);
                }
                if open(1, 0) {
                    draw_line(p.x + z, p.y, p.x + z, p.y + z, edge, c);
                }
            }
        }

        // Ore crystals.
        for y in y0..y1 {
            for x in x0..x1 {
                let Tile::Ore(n) = world.tile(x, y) else { continue };
                self.draw_ore(cam, x, y, n);
            }
        }

        // Factories.
        for f in &world.factories {
            let c = cam.w2s(tile_center(f.x, f.y));
            if c.x < -z * 2.0 || c.y < -z * 2.0 || c.x > screen_width() + z * 2.0 || c.y > screen_height() + z * 2.0 {
                continue;
            }
            let pc = if f.enabled { player_color(f.owner) } else { scale_rgb(player_color(f.owner), 0.45) };
            let r = z * 0.46;
            let hex = regular(c, r, 6, PI / 6.0);
            poly_fill(&hex, with_alpha(pc, 0.13));
            poly_lines(&hex, (z * 0.07).max(1.5), pc);
            let inner = regular(c, r * 0.55, 6, PI / 6.0);
            poly_lines(&inner, (z * 0.03).max(1.0), with_alpha(pc, 0.5));
            let tri = regular(c, r * 0.42, 3, t * 1.2 + f.id as f32);
            poly_lines(&tri, (z * 0.04).max(1.0), mix(pc, WHITE, 0.3));
            draw_circle(c.x, c.y, r * 0.13, mix(pc, WHITE, 0.6));
            if f.enabled {
                let prog = 1.0 - f.timer as f32 / SPAWN_INTERVAL as f32;
                arc(c, r * 1.22, -PI / 2.0, -PI / 2.0 + TAU * prog, (z * 0.04).max(1.0), with_alpha(pc, 0.55));
            }
            if f.hp < FACTORY_HP {
                let w = z * 0.9;
                let frac = f.hp as f32 / FACTORY_HP as f32;
                draw_rectangle(c.x - w / 2.0, c.y + r * 1.35, w, z * 0.08, with_alpha(BLACK, 0.7));
                draw_rectangle(c.x - w / 2.0, c.y + r * 1.35, w * frac, z * 0.08, mix(pal::BAD, pal::GOOD, frac));
            }
            if hl.factory == Some(f.id) {
                let rot = t * 0.8;
                for i in 0..6 {
                    let a0 = rot + i as f32 * TAU / 6.0;
                    arc(c, r * 1.5, a0, a0 + 0.6, 2.0, WHITE);
                }
            }
        }

        // Drones.
        let a = smooth(alpha);
        for d in &world.drones {
            let p = vec2(d.prev_x as f32, d.prev_y as f32).lerp(vec2(d.x as f32, d.y as f32), a) + vec2(0.5, 0.5);
            let s = cam.w2s(p);
            if s.x < -z || s.y < -z || s.x > screen_width() + z || s.y > screen_height() + z {
                continue;
            }
            let ang = self.angles.get(&d.id).copied().unwrap_or(facing_angle(d.facing));
            let pc = player_color(d.owner);
            let size = z * 0.34;
            let tip = s + Vec2::from_angle(ang) * size;
            let l = s + Vec2::from_angle(ang + 2.45) * size * 0.85;
            let r = s + Vec2::from_angle(ang - 2.45) * size * 0.85;
            let back = s + Vec2::from_angle(ang + PI) * size * 0.35;
            let hurt = d.hp <= 4 && ((t * 12.0 + d.id as f32).sin() > 0.3);
            let body = if hurt { mix(pc, pal::BAD, 0.6) } else { pc };
            draw_triangle(tip, l, back, with_alpha(body, 0.85));
            draw_triangle(tip, back, r, with_alpha(body, 0.85));
            poly_lines(&[tip, l, back, r], (z * 0.035).max(1.0), mix(body, WHITE, 0.45));
            if d.cargo > 0 {
                let cr = size * (0.12 + 0.05 * d.cargo as f32);
                draw_circle(s.x, s.y, cr, pal::ORE);
            }
            if d.last == LastAction::Stall {
                draw_circle(s.x, s.y - size * 1.1, z * 0.06, pal::WARN);
            }
            if hl.drone == Some(d.id) {
                arc(s, size * 1.7, t * 2.0, t * 2.0 + TAU * 0.75, 2.0, WHITE);
            }
        }

        self.fx.draw(cam);

        // Build ghost.
        if let Some(((gx, gy), ok)) = hl.ghost {
            let c = cam.w2s(tile_center(gx, gy));
            let col = if ok { pal::GOOD } else { pal::BAD };
            let pulse = 0.6 + 0.4 * (t * 6.0).sin();
            poly_lines(&regular(c, z * 0.46, 6, PI / 6.0), 2.0, with_alpha(col, pulse));
            poly_fill(&regular(c, z * 0.46, 6, PI / 6.0), with_alpha(col, 0.12));
        }
    }

    fn draw_ore(&self, cam: &Cam, x: i32, y: i32, n: u16) {
        let z = cam.zoom;
        let c = cam.w2s(tile_center(x, y));
        let h = hash2(x, y);
        let full = (n as f32 / ORE_MAX as f32).clamp(0.15, 1.3);
        let pulse = 0.75 + 0.25 * (self.time * 2.0 + h * 20.0).sin();
        let r = z * (0.14 + 0.16 * full.min(1.0));
        let rot = PI / 4.0 + (h - 0.5) * 0.5;
        let main = regular(c, r, 4, rot);
        poly_fill(&main, with_alpha(pal::ORE, 0.22 * pulse));
        poly_lines(&main, (z * 0.05).max(1.0), with_alpha(pal::ORE, 0.95 * pulse));
        let off = Vec2::from_angle(h * TAU) * z * 0.28;
        let sat = regular(c + off, r * 0.45, 4, rot + 0.3);
        poly_lines(&sat, (z * 0.035).max(1.0), with_alpha(pal::ORE, 0.8 * pulse));
        if full > 0.6 {
            let sat2 = regular(c - off.perp() * 0.9, r * 0.38, 4, rot - 0.2);
            poly_lines(&sat2, (z * 0.03).max(1.0), with_alpha(pal::ORE, 0.7 * pulse));
        }
        draw_circle(c.x, c.y, r * 0.18, with_alpha(pal::ORE_CORE, pulse));
    }

    /// Dots on every tile where `owner` could place a factory (range only).
    fn draw_build_range(&self, world: &World, cam: &Cam, owner: u8, (x0, y0, x1, y1): (i32, i32, i32, i32)) {
        use game_of_drones::rules::BUILD_RANGE;
        let anchors: Vec<(i32, i32)> = world
            .factories
            .iter()
            .filter(|f| f.owner == owner)
            .map(|f| (f.x, f.y))
            .chain(world.drones.iter().filter(|d| d.owner == owner).map(|d| (d.x, d.y)))
            .collect();
        let col = with_alpha(pal::GOOD, 0.22);
        for y in y0..y1 {
            for x in x0..x1 {
                if world.tile(x, y) != Tile::Floor {
                    continue;
                }
                if anchors.iter().any(|(ax, ay)| (ax - x).abs() + (ay - y).abs() <= BUILD_RANGE) {
                    let c = cam.w2s(tile_center(x, y));
                    draw_circle(c.x, c.y, (cam.zoom * 0.06).max(1.0), col);
                }
            }
        }
    }
}

/// Small overview of the whole map. Returns the clicked world position.
pub fn draw_minimap(world: &World, cam: &Cam, r: Rect, mouse: Vec2, clicked: bool) -> Option<Vec2> {
    draw_rectangle(r.x, r.y, r.w, r.h, pal::PANEL);
    let sx = r.w / world.w as f32;
    let sy = r.h / world.h as f32;
    for y in 0..world.h {
        for x in 0..world.w {
            let col = match world.tile(x, y) {
                Tile::Wall => scale_rgb(pal::WALL_EDGE, 0.35),
                Tile::Ore(_) => with_alpha(pal::ORE, 0.8),
                Tile::Floor => continue,
            };
            draw_rectangle(r.x + x as f32 * sx, r.y + y as f32 * sy, sx + 0.3, sy + 0.3, col);
        }
    }
    for d in &world.drones {
        draw_rectangle(r.x + d.x as f32 * sx, r.y + d.y as f32 * sy, sx.max(1.5), sy.max(1.5), player_color(d.owner));
    }
    for f in &world.factories {
        let c = vec2(r.x + (f.x as f32 + 0.5) * sx, r.y + (f.y as f32 + 0.5) * sy);
        draw_rectangle(c.x - 3.0, c.y - 3.0, 6.0, 6.0, player_color(f.owner));
    }
    // Camera viewport.
    let tl = cam.s2w(vec2(0.0, 0.0));
    let br = cam.s2w(vec2(screen_width(), screen_height()));
    let vx = (r.x + tl.x * sx).max(r.x);
    let vy = (r.y + tl.y * sy).max(r.y);
    let vw = (r.x + br.x * sx).min(r.x + r.w) - vx;
    let vh = (r.y + br.y * sy).min(r.y + r.h) - vy;
    if vw > 0.0 && vh > 0.0 {
        draw_rectangle_lines(vx, vy, vw, vh, 1.5, with_alpha(WHITE, 0.7));
    }
    draw_rectangle_lines(r.x, r.y, r.w, r.h, 1.5, pal::PANEL_EDGE);
    gfx::text("CARTE", r.x + 6.0, r.y + 4.0, 12.0, pal::DIM);
    if clicked && r.contains(mouse) {
        return Some(vec2((mouse.x - r.x) / sx, (mouse.y - r.y) / sy));
    }
    None
}
