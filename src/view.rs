//! Draws the world: starfield, asteroids, crystals, factories, dronoids and
//! effects. World coordinates are in units (floats here, fixed-point in
//! the simulation).

use std::collections::HashMap;
use std::f32::consts::{PI, TAU};

use macroquad::prelude::*;

use dronoids::rules::*;
use dronoids::sim::fog::FogMap;
use dronoids::sim::{ore_radius, Doing, Event, World};

use crate::fx::Fx;
use crate::gfx::{arc, mix, pal, player_color, poly_fill, poly_lines, regular, scale_rgb, with_alpha};

#[derive(Clone, Copy, Debug)]
pub struct Cam {
    pub center: Vec2,
    /// Pixels per world unit.
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

    /// What a `Sight` needs to know about: the view and the map around it.
    pub fn sight_area(&self) -> Rect {
        let tl = self.s2w(Vec2::ZERO);
        let br = self.s2w(vec2(screen_width(), screen_height()));
        let m = MAP_SPAN / 2.0 + 8.0;
        let (x0, y0) = (tl.x.min(self.center.x - m), tl.y.min(self.center.y - m));
        let (x1, y1) = (br.x.max(self.center.x + m), br.y.max(self.center.y + m));
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }

    fn visible(&self, p: Vec2, margin: f32) -> bool {
        let s = self.w2s(p);
        let m = margin * self.zoom;
        s.x > -m && s.y > -m && s.x < screen_width() + m && s.y < screen_height() + m
    }
}

/// Fixed-point to world units.
pub fn fp(v: i32) -> f32 {
    v as f32 / FP as f32
}

pub fn fpv(x: i32, y: i32) -> Vec2 {
    vec2(fp(x), fp(y))
}

/// Compass degrees to a screen angle in radians.
pub fn heading_rad(deg: i32) -> f32 {
    (deg as f32 - 90.0).to_radians()
}

fn hash(a: u32, b: u32) -> f32 {
    let mut h = a.wrapping_mul(374_761_393) ^ b.wrapping_mul(668_265_263);
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    (h & 0xffff) as f32 / 65535.0
}

/// What the local player can see right now, and has ever seen.
/// The simulation is fully replicated on every client, so the fog of war is
/// a matter of what we choose to draw.
pub struct Sight {
    me: Option<u8>,
    /// The player's map of explored space, around where we are looking.
    explored: FogMap,
    /// Vision circles: centre and radius, in world units.
    pub sources: Vec<(Vec2, f32)>,
}

impl Sight {
    /// No fog at all (title screen).
    pub fn all() -> Sight {
        Sight { me: None, explored: FogMap::default(), sources: vec![] }
    }

    pub fn of(world: &World, me: u8, alpha: f32, area: Rect) -> Sight {
        let mut sources: Vec<(Vec2, f32)> = vec![];
        for f in world.factories.iter().filter(|f| f.owner == me) {
            sources.push((fpv(f.x, f.y), fp(VISION_FACTORY)));
        }
        let a = alpha.clamp(0.0, 1.0);
        for d in world.dronoids.iter().filter(|d| d.owner == me) {
            sources.push((fpv(d.prev_x, d.prev_y).lerp(fpv(d.x, d.y), a), fp(VISION_DRONOID)));
        }
        if let Some(p) = world.players.get(me as usize)
            && p.alive
            && world.is_landing(me)
        {
            // Before landing, the arrival scan shows where to settle.
            sources.push((fpv(p.home.0, p.home.1), fp(ARRIVAL_SCAN)));
        }
        let cell = fp(FOG_CELL);
        let (x0, y0) = ((area.x / cell).floor() as i32, (area.y / cell).floor() as i32);
        let (x1, y1) = (((area.x + area.w) / cell).ceil() as i32, ((area.y + area.h) / cell).ceil() as i32);
        let explored = world.players.get(me as usize).map(|p| p.seen.around(x0, y0, x1, y1)).unwrap_or_default();
        Sight { me: Some(me), explored, sources }
    }

    pub fn me(&self) -> Option<u8> {
        self.me
    }

    pub fn fogged(&self) -> bool {
        self.me.is_some()
    }

    /// Seen right now.
    pub fn visible(&self, p: Vec2) -> bool {
        self.me.is_none() || self.sources.iter().any(|(c, r)| c.distance_squared(p) <= r * r)
    }

    /// Seen at some point (terrain is remembered).
    pub fn explored(&self, p: Vec2) -> bool {
        if self.me.is_none() || self.visible(p) {
            return true;
        }
        self.explored_cell((p.x / fp(FOG_CELL)).floor() as i32, (p.y / fp(FOG_CELL)).floor() as i32)
    }

    pub fn explored_cell(&self, cx: i32, cy: i32) -> bool {
        self.explored.get(cx, cy)
    }

    /// Is this unit drawn? Our own always, others only in sight.
    pub fn shows(&self, owner: u8, p: Vec2) -> bool {
        self.me.is_none_or(|me| me == owner) || self.visible(p)
    }

    /// Paint the fog mask, in screen space: unknown space stays dark,
    /// explored space is veiled, what we see right now is clear.
    pub fn paint_mask(&self, cam: &Cam) {
        let z = cam.zoom;
        let cell = fp(FOG_CELL);
        let tl = cam.s2w(vec2(0.0, 0.0));
        let br = cam.s2w(vec2(screen_width(), screen_height()));
        let (x0, y0) = ((tl.x / cell).floor() as i32 - 1, (tl.y / cell).floor() as i32 - 1);
        let (x1, y1) = ((br.x / cell).ceil() as i32 + 1, (br.y / cell).ceil() as i32 + 1);
        for cy in y0..=y1 {
            for cx in x0..=x1 {
                if self.explored_cell(cx, cy) {
                    let c = cam.w2s(vec2((cx as f32 + 0.5) * cell, (cy as f32 + 0.5) * cell));
                    draw_circle(c.x, c.y, cell * z * 0.78, FOG_EXPLORED);
                }
            }
        }
        for (c, r) in &self.sources {
            let s = cam.w2s(*c);
            draw_circle(s.x, s.y, r * z, WHITE);
        }
    }
}

/// How dark space is where we have never been, and how veiled where we have.
pub const FOG_UNKNOWN: Color = Color::new(0.025, 0.03, 0.055, 1.0);
pub const FOG_EXPLORED: Color = Color::new(0.33, 0.37, 0.52, 1.0);

pub struct Highlight {
    pub factory: Option<u32>,
    pub dronoid: Option<u32>,
    /// Build ghost: world position and validity.
    pub ghost: Option<(Vec2, bool)>,
    pub build_range_for: Option<u8>,
    /// Target marker of the selected factory.
    pub target: Option<Vec2>,
    /// Suggested place for a first factory.
    pub landing: Option<Vec2>,
    pub me: u8,
}

pub struct WorldView {
    pub fx: Fx,
    angles: HashMap<u32, f32>,
    pub time: f32,
    pub shake: f32,
    stars: Vec<(Vec2, f32, f32)>,
}

impl Default for WorldView {
    fn default() -> Self {
        let stars = (0..700)
            .map(|i| {
                let p = vec2(hash(i, 1), hash(i, 2));
                let depth = 0.04 + 0.22 * hash(i, 3).powi(2);
                (p, depth, 0.25 + 0.75 * hash(i, 4))
            })
            .collect();
        WorldView { fx: Fx::default(), angles: HashMap::new(), time: 0.0, shake: 0.0, stars }
    }
}

impl WorldView {
    /// Turn simulation events into eye candy.
    pub fn on_events(&mut self, world: &World, speed: f32, sight: &Sight) {
        let heavy = speed > 4.0;
        for e in &world.events {
            // Nothing to see behind the fog.
            let seen = match *e {
                Event::Spawn { x, y, .. }
                | Event::DronoidDestroyed { x, y, .. }
                | Event::FactoryDestroyed { x, y, .. }
                | Event::FactoryBuilt { x, y, .. }
                | Event::OreGrown { x, y } => sight.visible(fpv(x, y)),
                Event::Mine { tx, ty, .. } | Event::Drop { tx, ty, .. } => sight.visible(fpv(tx, ty)),
                Event::Attack { x, y, tx, ty, .. } => sight.visible(fpv(x, y)) || sight.visible(fpv(tx, ty)),
                Event::PlayerJoined { player } => {
                    world.players.get(player as usize).is_some_and(|p| sight.visible(fpv(p.home.0, p.home.1)))
                }
                Event::PlayerEliminated { .. } => false,
                Event::QuestDone { player, .. } => sight.me() == Some(player),
            };
            if !seen {
                continue;
            }
            match *e {
                Event::Spawn { owner, x, y } => {
                    self.fx.ring(fpv(x, y), player_color(owner), 0.8, 0.5);
                }
                Event::Mine { tx, ty, depleted, .. } => {
                    if !heavy {
                        self.fx.dots(fpv(tx, ty), pal::ORE, 3, 1.2, 0.6);
                    }
                    if depleted {
                        self.fx.burst(fpv(tx, ty), pal::ORE, 14, 3.0, 0.8);
                    }
                }
                Event::Drop { x, y, tx, ty, .. } => {
                    self.fx.beam(fpv(x, y), fpv(tx, ty), pal::ORE_CORE, 0.08, 0.35);
                    self.fx.ring(fpv(tx, ty), pal::ORE, 1.0, 0.4);
                }
                Event::Attack { owner, x, y, tx, ty } => {
                    let c = mix(player_color(owner), WHITE, 0.35);
                    self.fx.beam(fpv(x, y), fpv(tx, ty), c, 0.09, 0.25);
                    if !heavy {
                        self.fx.burst(fpv(tx, ty), c, 4, 2.5, 0.3);
                    }
                }
                Event::DronoidDestroyed { owner, x, y } => {
                    let at = fpv(x, y);
                    self.fx.burst(at, player_color(owner), 16, 4.0, 0.9);
                    self.fx.ring(at, player_color(owner), 1.4, 0.5);
                }
                Event::FactoryDestroyed { owner, x, y } => {
                    let at = fpv(x, y);
                    self.fx.burst(at, player_color(owner), 60, 7.0, 1.4);
                    self.fx.burst(at, WHITE, 20, 5.0, 0.9);
                    self.fx.ring(at, player_color(owner), 5.0, 1.2);
                    self.fx.ring(at, WHITE, 3.0, 0.7);
                    self.shake = self.shake.max(0.3);
                }
                Event::FactoryBuilt { owner, x, y } => {
                    self.fx.ring(fpv(x, y), player_color(owner), 3.5, 0.9);
                    self.fx.dots(fpv(x, y), player_color(owner), 24, 3.0, 0.9);
                }
                Event::PlayerJoined { player } => {
                    if let Some(p) = world.players.get(player as usize) {
                        self.fx.ring(fpv(p.home.0, p.home.1), player_color(player), 6.0, 1.5);
                    }
                }
                Event::PlayerEliminated { .. } => {}
                Event::QuestDone { player, reward, .. } => {
                    // The reward lands at home.
                    if let Some(f) = world.factories.iter().find(|f| f.owner == player) {
                        let at = fpv(f.x, f.y);
                        self.fx.ring(at, pal::ORE, 6.0, 1.4);
                        if reward > 0 {
                            self.fx.dots(at, pal::ORE_CORE, 30, 3.5, 1.2);
                        }
                    }
                }
                Event::OreGrown { x, y } => {
                    self.fx.ring(fpv(x, y), pal::ORE, 5.0, 2.5);
                    self.fx.dots(fpv(x, y), pal::ORE_CORE, 18, 2.0, 1.6);
                }
            }
        }
    }

    pub fn update(&mut self, dt: f32, world: &World) {
        self.time += dt;
        self.fx.update(dt);
        self.shake = (self.shake - dt).max(0.0);
        let k = (dt * 10.0).min(1.0);
        for d in &world.dronoids {
            let target = heading_rad(d.heading);
            let a = self.angles.entry(d.id).or_insert(target);
            let mut diff = (target - *a) % TAU;
            if diff > PI {
                diff -= TAU;
            } else if diff < -PI {
                diff += TAU;
            }
            *a += diff * k;
        }
        if self.angles.len() > world.dronoids.len() * 2 + 64 {
            let alive: std::collections::HashSet<u32> = world.dronoids.iter().map(|d| d.id).collect();
            self.angles.retain(|id, _| alive.contains(id));
        }
    }

    pub fn shake_offset(&self) -> Vec2 {
        if self.shake <= 0.0 {
            return Vec2::ZERO;
        }
        let s = self.shake * 0.8;
        vec2((self.time * 71.0).sin() * s, (self.time * 53.0).cos() * s)
    }

    /// World position of a dronoid, interpolated between ticks.
    pub fn dronoid_pos(world: &World, id: u32, alpha: f32) -> Option<Vec2> {
        let d = world.dronoid(id)?;
        Some(fpv(d.prev_x, d.prev_y).lerp(fpv(d.x, d.y), alpha.clamp(0.0, 1.0)))
    }

    fn draw_stars(&self, cam: &Cam) {
        let (sw, sh) = (screen_width(), screen_height());
        let tile = vec2(1800.0, 1100.0);
        for (p, depth, b) in &self.stars {
            let off = cam.center * cam.zoom * *depth;
            let mut s = *p * tile - off;
            s.x = s.x.rem_euclid(tile.x);
            s.y = s.y.rem_euclid(tile.y);
            let mut y = s.y;
            while y < sh {
                let mut x = s.x;
                while x < sw {
                    let tw = 0.6 + 0.4 * (self.time * (1.0 + b * 2.0) + p.x * 50.0).sin();
                    draw_circle(x, y, 0.6 + depth * 5.0, Color::new(0.6, 0.75, 1.0, b * tw * 0.55));
                    x += tile.x;
                }
                y += tile.y;
            }
        }
    }

    pub fn draw(&self, world: &World, cam: &Cam, alpha: f32, hl: &Highlight, sight: &Sight) {
        let z = cam.zoom;
        let t = self.time;
        let alpha = alpha.clamp(0.0, 1.0);
        self.draw_stars(cam);

        if let Some(owner) = hl.build_range_for {
            let col = with_alpha(pal::GOOD, 0.10);
            for f in world.factories.iter().filter(|f| f.owner == owner) {
                let c = cam.w2s(fpv(f.x, f.y));
                draw_circle(c.x, c.y, fp(BUILD_RANGE + 2 * FACTORY_RADIUS) * z, col);
            }
            for d in world.dronoids.iter().filter(|d| d.owner == owner) {
                let c = cam.w2s(fpv(d.x, d.y));
                draw_circle(c.x, c.y, fp(BUILD_RANGE) * z, with_alpha(pal::GOOD, 0.04));
            }
        }

        // Whatever the view reaches of the terrain.
        let tl = cam.s2w(Vec2::ZERO);
        let br = cam.s2w(vec2(screen_width(), screen_height()));
        let fixed = |v: f32| (v * FP as f32) as i32;
        let (x0, y0, x1, y1) = (fixed(tl.x - 9.0), fixed(tl.y - 9.0), fixed(br.x + 9.0), fixed(br.y + 9.0));

        // Asteroids.
        let edge = (z * 0.08).clamp(1.2, 3.0);
        world.rocks_in(x0, y0, x1, y1, |_, r| {
            let c = fpv(r.x, r.y);
            let rad = fp(r.r);
            if !cam.visible(c, rad + 1.0) || !(sight.explored(c) || sight.explored(c + vec2(rad, 0.0)) || sight.explored(c - vec2(rad, 0.0)) || sight.explored(c + vec2(0.0, rad)) || sight.explored(c - vec2(0.0, rad))) {
                return;
            }
            let sc = cam.w2s(c);
            if z * rad > 6.0 {
                let pts = rock_outline(r.seed);
                let outer: Vec<Vec2> = pts.iter().map(|p| sc + *p * rad * z).collect();
                let inner: Vec<Vec2> = pts.iter().map(|p| sc + *p * rad * z * 0.55).collect();
                let shade = 0.8 + 0.4 * hash(r.seed, 7);
                for k in 0..outer.len() {
                    draw_triangle(sc, outer[k], outer[(k + 1) % outer.len()], scale_rgb(pal::WALL_FILL, shade));
                }
                poly_lines(&inner, 1.0, with_alpha(pal::WALL_EDGE, 0.18));
                poly_lines(&outer, edge, pal::WALL_EDGE);
            } else {
                draw_circle(sc.x, sc.y, rad * z, pal::WALL_FILL);
                draw_circle_lines(sc.x, sc.y, rad * z, 1.0, pal::WALL_EDGE);
            }
        });

        // Crystals.
        world.ores_in(x0, y0, x1, y1, |o| {
            let c = fpv(o.x, o.y);
            if !cam.visible(c, 1.5) || !sight.explored(c) {
                return;
            }
            let sc = cam.w2s(c);
            let r = fp(ore_radius(o.amount)) * z;
            let id = (o.id as u32) ^ ((o.id >> 32) as u32).wrapping_mul(2_654_435_761);
            let h = hash(id, 3);
            let pulse = 0.75 + 0.25 * (t * 1.6 + h * 20.0).sin();
            if z < 5.0 {
                draw_circle(sc.x, sc.y, r.max(1.0), with_alpha(pal::ORE, 0.8 * pulse));
                return;
            }
            // A cluster of crystal shards pointing outwards.
            let base = h * TAU;
            let shards = 3 + (id % 3) as usize;
            for k in 0..shards {
                let a = base + k as f32 * TAU / shards as f32 + hash(id, k as u32) * 0.8;
                let len = r * (0.9 + 0.6 * hash(id, 10 + k as u32));
                let dir = Vec2::from_angle(a);
                let side = dir.perp() * r * 0.28;
                let root = sc - dir * r * 0.15;
                let shard = [root + dir * len, root + side, root - dir * r * 0.25, root - side];
                poly_fill(&shard, with_alpha(pal::ORE, 0.2 * pulse));
                poly_lines(&shard, (z * 0.04).max(1.0), with_alpha(pal::ORE, 0.95 * pulse));
            }
            draw_circle(sc.x, sc.y, r * 0.2, with_alpha(pal::ORE_CORE, pulse));
        });

        // Mining beams (continuous while a dronoid digs).
        for d in &world.dronoids {
            if let Doing::Mine { tx, ty } = d.doing {
                let p = fpv(d.x, d.y);
                if !cam.visible(p, 3.0) || !sight.shows(d.owner, p) {
                    continue;
                }
                let (s, e) = (cam.w2s(p), cam.w2s(fpv(tx, ty)));
                let flicker = 0.6 + 0.4 * (t * 30.0 + d.id as f32).sin();
                draw_line(s.x, s.y, e.x, e.y, (z * 0.05).max(1.0), with_alpha(pal::ORE, 0.7 * flicker));
            }
        }

        // Factories.
        for f in &world.factories {
            let c = fpv(f.x, f.y);
            if !cam.visible(c, 4.0) || !sight.shows(f.owner, c) {
                continue;
            }
            let sc = cam.w2s(c);
            let pc = if f.enabled { player_color(f.owner) } else { scale_rgb(player_color(f.owner), 0.45) };
            let r = fp(FACTORY_RADIUS) * z;
            let hex = regular(sc, r, 6, PI / 6.0);
            poly_fill(&hex, with_alpha(pc, 0.13));
            poly_lines(&hex, (z * 0.08).max(1.5), pc);
            poly_lines(&regular(sc, r * 0.55, 6, PI / 6.0), (z * 0.03).max(1.0), with_alpha(pc, 0.5));
            poly_lines(&regular(sc, r * 0.42, 3, t * 0.8 + f.id as f32), (z * 0.04).max(1.0), mix(pc, WHITE, 0.3));
            draw_circle(sc.x, sc.y, r * 0.13, mix(pc, WHITE, 0.6));
            if f.enabled && f.timer > 0 {
                let prog = 1.0 - f.timer as f32 / SPAWN_TICKS as f32;
                arc(sc, r * 1.25, -PI / 2.0, -PI / 2.0 + TAU * prog, (z * 0.04).max(1.0), with_alpha(pc, 0.5));
            }
            if f.hp < FACTORY_HP {
                let w = r * 1.8;
                let frac = f.hp as f32 / FACTORY_HP as f32;
                draw_rectangle(sc.x - w / 2.0, sc.y + r * 1.4, w, (z * 0.12).max(3.0), with_alpha(BLACK, 0.7));
                draw_rectangle(sc.x - w / 2.0, sc.y + r * 1.4, w * frac, (z * 0.12).max(3.0), mix(pal::BAD, pal::GOOD, frac));
            }
            if world.shielded(f.owner) {
                // Newcomer protection: a faint shimmering bubble.
                let shimmer = 0.10 + 0.05 * (t * 1.7 + f.id as f32).sin();
                draw_circle_lines(sc.x, sc.y, r * 2.3, 1.0, with_alpha(mix(pc, WHITE, 0.4), shimmer * 2.0));
                draw_circle(sc.x, sc.y, r * 2.3, with_alpha(pc, shimmer * 0.35));
            }
            if hl.factory == Some(f.id) {
                for i in 0..6 {
                    let a0 = t * 0.8 + i as f32 * TAU / 6.0;
                    arc(sc, r * 1.6, a0, a0 + 0.6, 2.0, WHITE);
                }
            }
        }

        // Dronoids.
        for d in &world.dronoids {
            let p = fpv(d.prev_x, d.prev_y).lerp(fpv(d.x, d.y), alpha);
            if !cam.visible(p, 1.0) || !sight.shows(d.owner, p) {
                continue;
            }
            let s = cam.w2s(p);
            let pc = player_color(d.owner);
            let size = fp(DRONOID_RADIUS) * z * 1.15;
            if size < 2.0 {
                draw_rectangle(s.x - 1.0, s.y - 1.0, 2.0, 2.0, pc);
                continue;
            }
            let ang = self.angles.get(&d.id).copied().unwrap_or(heading_rad(d.heading));
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
                draw_circle(s.x, s.y, size * (0.12 + 0.05 * d.cargo as f32), pal::ORE);
            }
            if let Doing::Move { .. } = d.doing {
                // Engine glow.
                let tail = s + Vec2::from_angle(ang + PI) * size * 0.75;
                draw_circle(tail.x, tail.y, size * 0.18, with_alpha(mix(pc, WHITE, 0.5), 0.8));
            }
            if d.doing == Doing::Stall {
                draw_circle(s.x, s.y - size * 1.3, (z * 0.07).max(1.5), pal::WARN);
            }
            if hl.dronoid == Some(d.id) {
                arc(s, size * 1.8, t * 2.0, t * 2.0 + TAU * 0.75, 2.0, WHITE);
            }
        }

        self.fx.draw(cam);

        if let Some(target) = hl.target {
            let c = cam.w2s(target);
            let rr = 14.0 + 3.0 * (t * 4.0).sin();
            let col = player_color(hl.me);
            draw_circle_lines(c.x, c.y, rr, 2.0, col);
            for k in 0..4 {
                let d = Vec2::from_angle(k as f32 * PI / 2.0);
                let (a, b) = (c + d * (rr - 6.0), c + d * (rr + 8.0));
                draw_line(a.x, a.y, b.x, b.y, 2.0, col);
            }
        }

        if let Some(p) = hl.landing {
            let c = cam.w2s(p);
            let col = player_color(hl.me);
            let rr = fp(FACTORY_RADIUS) * z * 2.0;
            for i in 0..3 {
                let k = (t * 0.5 + i as f32 / 3.0) % 1.0;
                draw_circle_lines(c.x, c.y, rr * (1.0 + k * 2.0), 1.5, with_alpha(col, 0.5 * (1.0 - k)));
            }
            draw_circle(c.x, c.y, 3.0, col);
        }

        if let Some((p, ok)) = hl.ghost {
            let c = cam.w2s(p);
            let col = if ok { pal::GOOD } else { pal::BAD };
            let pulse = 0.6 + 0.4 * (t * 6.0).sin();
            let hex = regular(c, fp(FACTORY_RADIUS) * z, 6, PI / 6.0);
            poly_lines(&hex, 2.0, with_alpha(col, pulse));
            poly_fill(&hex, with_alpha(col, 0.12));
        }
    }
}

/// The irregular outline of an asteroid, around the unit circle.
fn rock_outline(seed: u32) -> Vec<Vec2> {
    let n = 9 + (seed % 5) as usize;
    (0..n)
        .map(|i| {
            let a = i as f32 / n as f32 * TAU + hash(seed, i as u32) * 0.4;
            Vec2::from_angle(a) * (0.78 + 0.27 * hash(seed, 100 + i as u32))
        })
        .collect()
}

/// How many units across the map shows.
pub const MAP_SPAN: f32 = 360.0;

/// A small round sonar of the surroundings, centred on where we are looking
/// and fading out at its rim: there is no overview of a world this large.
/// Returns the clicked world position.
pub fn draw_minimap(world: &World, cam: &Cam, r: Rect, sight: &Sight, mouse: Vec2, clicked: bool) -> Option<Vec2> {
    let me = sight.me().unwrap_or(u8::MAX);
    let middle = r.point() + r.size() / 2.0;
    let radius = r.w.min(r.h) / 2.0;
    let k = 2.0 * radius / MAP_SPAN;
    for i in 0..8 {
        draw_circle(middle.x, middle.y, radius * (1.36 - i as f32 * 0.1), with_alpha(pal::BG, 0.16));
    }
    let at = |p: Vec2| middle + (p - cam.center) * k;
    let fade = |p: Vec2| ((1.0 - p.distance(middle) / radius) * 4.0).clamp(0.0, 1.0);
    // Explored space shows as a faint wash; the rest stays black. (Opaque:
    // overlaps draw no grid.)
    if sight.fogged() {
        let cell = fp(FOG_CELL);
        let side = cell * k;
        let (cx, cy) = ((cam.center.x / cell).floor() as i32, (cam.center.y / cell).floor() as i32);
        let half = (MAP_SPAN / 2.0 / cell).ceil() as i32;
        let dark = Color::new(0.012, 0.016, 0.04, 1.0);
        let light = Color::new(0.075, 0.1, 0.19, 1.0);
        for j in cy - half..=cy + half {
            for i in cx - half..=cx + half {
                if sight.explored_cell(i, j) {
                    let p = at(vec2(i as f32 * cell, j as f32 * cell));
                    let t = fade(p + Vec2::splat(side / 2.0));
                    if t > 0.0 {
                        draw_rectangle(p.x, p.y, side + 0.3, side + 0.3, mix(dark, light, t));
                    }
                }
            }
        }
    }
    let reach = (MAP_SPAN / 2.0 * FP as f32) as i32;
    let (mx, my) = ((cam.center.x * FP as f32) as i32, (cam.center.y * FP as f32) as i32);
    world.rocks_in(mx - reach, my - reach, mx + reach, my + reach, |_, rock| {
        let c = fpv(rock.x, rock.y);
        let (p, t) = (at(c), fade(at(c)));
        if t > 0.0 && sight.explored(c) {
            draw_circle(p.x, p.y, (fp(rock.r) * k).max(0.7), with_alpha(scale_rgb(pal::WALL_EDGE, 0.45), t));
        }
    });
    world.ores_in(mx - reach, my - reach, mx + reach, my + reach, |o| {
        let c = fpv(o.x, o.y);
        let (p, t) = (at(c), fade(at(c)));
        if t > 0.0 && sight.explored(c) {
            draw_rectangle(p.x, p.y, 1.2, 1.2, with_alpha(pal::ORE, 0.7 * t));
        }
    });
    for d in world.dronoids.iter().filter(|d| sight.shows(d.owner, fpv(d.x, d.y))) {
        let (p, t) = (at(fpv(d.x, d.y)), fade(at(fpv(d.x, d.y))));
        if t > 0.0 {
            draw_rectangle(p.x - 0.7, p.y - 0.7, 1.6, 1.6, with_alpha(player_color(d.owner), t));
        }
    }
    for f in world.factories.iter().filter(|f| sight.shows(f.owner, fpv(f.x, f.y))) {
        let (p, t) = (at(fpv(f.x, f.y)), fade(at(fpv(f.x, f.y))));
        if t > 0.0 {
            let s = if f.owner == me { 5.0 } else { 4.0 };
            draw_rectangle(p.x - s / 2.0, p.y - s / 2.0, s, s, with_alpha(player_color(f.owner), t));
        }
    }
    // Where we are looking.
    draw_circle(middle.x, middle.y, 5.0, with_alpha(WHITE, 0.12));
    draw_circle(middle.x, middle.y, 2.0, with_alpha(WHITE, 0.75));
    (clicked && mouse.distance(middle) <= radius).then(|| cam.center + (mouse - middle) / k)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dronoids::programs;
    use dronoids::sim::Command;

    #[test]
    fn enemies_only_show_within_sight() {
        let mut w = World::new(4);
        let m = programs::builtin("mineur").unwrap();
        for name in ["moi", "autre"] {
            let id = w.join(name, false).unwrap();
            let (hx, hy) = w.players[id as usize].home;
            w.step(&[(id, Command::PlaceFactory { x: hx, y: hy, name: m.name.into(), source: m.source.into() })]);
        }
        for _ in 0..5 {
            w.step(&[]);
        }
        let mine = w.factories.iter().find(|f| f.owner == 0).unwrap().clone();
        let theirs = w.factories.iter().find(|f| f.owner == 1).unwrap().clone();
        let sight = Sight::of(&w, 0, 1.0, Rect::new(-3000.0, -3000.0, 6000.0, 6000.0));
        assert!(sight.shows(0, fpv(mine.x, mine.y)));
        assert!(!sight.shows(1, fpv(theirs.x, theirs.y)), "a far colony hides in the fog");
        assert!(!sight.explored(fpv(theirs.x, theirs.y)));
        // An enemy dronoid parked next to our factory is seen.
        let di = w.dronoids.iter().position(|d| d.owner == 1).unwrap();
        let near = fpv(mine.x + 5 * FP, mine.y);
        w.dronoids[di].x = mine.x + 5 * FP;
        w.dronoids[di].y = mine.y;
        w.dronoids[di].prev_x = w.dronoids[di].x;
        w.dronoids[di].prev_y = w.dronoids[di].y;
        let sight = Sight::of(&w, 0, 1.0, Rect::new(-3000.0, -3000.0, 6000.0, 6000.0));
        assert!(sight.shows(1, near));
        assert!(Sight::all().shows(1, fpv(theirs.x, theirs.y)), "the title screen sees everything");
    }
}
