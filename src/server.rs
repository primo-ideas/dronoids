//! World server. It owns the authoritative world, runs the computer
//! players, and each tick broadcasts the ordered list of commands; clients
//! replay them on their own copy (deterministic simulation), so only
//! commands travel — never the thousands of drones. A newcomer receives a
//! snapshot, then the frames. Clients report hashes; a client that drifts
//! is sent a fresh snapshot.
//!
//! Runs inside the game ("Héberger un monde") or headless (`god-server`).

use std::collections::BTreeMap;
use std::io;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::ai::{Bot, Level};
use crate::net::{ClientMsg, Conn, NetEvent, Responder, ServerMsg, WorldInfo, PROTOCOL_VERSION};
use crate::rules::TICKS_PER_SEC;
use crate::sim::{Command, World};

pub const HASH_EVERY: u64 = 50;
const SAVE_EVERY: Duration = Duration::from_secs(60);

pub const BOT_NAMES: &[&str] = &[
    "Orion", "Vega", "Lyra", "Rigel", "Deneb", "Sirius", "Altaïr", "Antarès", "Castor", "Pollux", "Capella", "Mira",
];

#[derive(Clone, Debug)]
pub struct ServerConfig {
    pub name: String,
    pub port: u16,
    pub seed: u64,
    pub bots: usize,
    pub level: Level,
    /// Where the world is saved (and loaded from, if it exists).
    pub save: Option<PathBuf>,
}

struct Client {
    conn: Conn<ClientMsg>,
    name: Option<String>,
    player: Option<u8>,
    joining: bool,
    gone: bool,
}

pub struct Server {
    cfg: ServerConfig,
    pub world: World,
    bots: Vec<Bot>,
    listener: TcpListener,
    clients: Vec<Client>,
    pending: Vec<(u8, Command)>,
    hashes: BTreeMap<u64, u64>,
    responder: Option<Responder>,
    last_save: Instant,
    pub log: Vec<String>,
}

pub fn bot_name(i: usize) -> String {
    let base = BOT_NAMES[i % BOT_NAMES.len()];
    if i < BOT_NAMES.len() { format!("IA {base}") } else { format!("IA {base} {}", i / BOT_NAMES.len() + 1) }
}

impl Server {
    pub fn new(cfg: ServerConfig) -> io::Result<Server> {
        let listener = TcpListener::bind(("0.0.0.0", cfg.port))?;
        listener.set_nonblocking(true)?;
        let mut log = vec![];
        let loaded = cfg.save.as_ref().filter(|p| p.exists()).and_then(|p| match std::fs::read(p) {
            Ok(bytes) => match World::from_bytes(&bytes) {
                Ok(w) => {
                    log.push(format!("monde chargé depuis {}", p.display()));
                    Some(w)
                }
                Err(e) => {
                    log.push(format!("sauvegarde illisible ({e}), nouveau monde"));
                    None
                }
            },
            Err(_) => None,
        });
        let world = loaded.unwrap_or_else(|| {
            let mut w = World::new(cfg.seed);
            for i in 0..cfg.bots {
                w.join(&bot_name(i), true);
            }
            w
        });
        let bots = world.players.iter().filter(|p| p.bot).map(|p| Bot::new(p.id, cfg.level)).collect();
        let responder = Responder::bind().ok();
        if responder.is_none() {
            log.push("découverte LAN indisponible (port déjà pris ?)".into());
        }
        Ok(Server {
            cfg,
            world,
            bots,
            listener,
            clients: vec![],
            pending: vec![],
            hashes: BTreeMap::new(),
            responder,
            last_save: Instant::now(),
            log,
        })
    }

    pub fn port(&self) -> u16 {
        self.listener.local_addr().map(|a| a.port()).unwrap_or(self.cfg.port)
    }

    pub fn info(&self) -> WorldInfo {
        WorldInfo {
            name: self.cfg.name.clone(),
            port: self.port(),
            players: self.world.players.iter().filter(|p| !p.bot).count() as u32,
            online: self.clients.iter().filter(|c| c.player.is_some()).count() as u32,
            bots: self.bots.len() as u32,
            tick: self.world.tick,
            version: PROTOCOL_VERSION,
        }
    }

    fn handle_messages(&mut self) {
        while let Ok((stream, addr)) = self.listener.accept() {
            match Conn::new(stream) {
                Ok(conn) => {
                    self.log.push(format!("connexion de {addr}"));
                    self.clients.push(Client { conn, name: None, player: None, joining: false, gone: false });
                }
                Err(e) => self.log.push(format!("connexion refusée : {e}")),
            }
        }
        for ci in 0..self.clients.len() {
            while let Some(ev) = self.clients[ci].conn.poll_one() {
                match ev {
                    NetEvent::Msg(ClientMsg::Hello { version, name }) => {
                        let name: String = name.trim().chars().filter(|c| !c.is_control()).take(24).collect();
                        let taken = self.clients.iter().any(|c| c.name.as_deref() == Some(name.as_str()) && !c.gone)
                            || self.world.player_by_name(&name).is_some_and(|p| p.bot);
                        let c = &mut self.clients[ci];
                        if version != PROTOCOL_VERSION {
                            c.conn.send(&ServerMsg::Refused("version du jeu différente".into()));
                            c.gone = true;
                        } else if name.is_empty() || taken {
                            c.conn.send(&ServerMsg::Refused(format!("le nom « {name} » est déjà utilisé")));
                            c.gone = true;
                        } else {
                            c.name = Some(name.clone());
                            c.joining = true;
                            self.pending.push((0, Command::Join { name, bot: false }));
                        }
                    }
                    NetEvent::Msg(ClientMsg::Cmd(cmd)) => {
                        let c = &self.clients[ci];
                        let Some(p) = c.player else { continue };
                        let allowed = match &cmd {
                            Command::Join { name, bot } => !bot && c.name.as_deref() == Some(name.as_str()),
                            _ => true,
                        };
                        if allowed {
                            self.pending.push((p, cmd));
                        }
                    }
                    NetEvent::Msg(ClientMsg::Hash { tick, hash }) => {
                        if let Some(&mine) = self.hashes.get(&tick)
                            && mine != hash {
                                let c = &mut self.clients[ci];
                                self.log.push(format!("{} désynchronisé au tick {tick}, renvoi du monde", c.name.clone().unwrap_or_default()));
                                c.conn.send(&ServerMsg::Resync { snapshot: self.world.to_bytes() });
                            }
                    }
                    NetEvent::Msg(ClientMsg::Bye) | NetEvent::Closed(_) => {
                        self.clients[ci].gone = true;
                        break;
                    }
                }
            }
        }
        let before = self.clients.len();
        self.clients.retain(|c| !c.gone);
        if self.clients.len() != before {
            self.log.push(format!("{} joueur(s) connecté(s)", self.clients.len()));
        }
    }

    /// One server tick.
    pub fn tick(&mut self) {
        self.handle_messages();
        for b in &mut self.bots {
            let p = b.player;
            self.pending.extend(b.think(&self.world).into_iter().map(|c| (p, c)));
        }
        let cmds = std::mem::take(&mut self.pending);
        let frame = ServerMsg::Frame { tick: self.world.tick, cmds: cmds.clone() };
        for c in self.clients.iter_mut().filter(|c| c.player.is_some()) {
            c.conn.send(&frame);
        }
        self.world.step(&cmds);
        if self.world.tick.is_multiple_of(HASH_EVERY) {
            self.hashes.insert(self.world.tick, self.world.state_hash());
            while self.hashes.len() > 40 {
                self.hashes.pop_first();
            }
        }
        // Welcome the players whose Join was just applied.
        let mut snapshot: Option<Vec<u8>> = None;
        for c in self.clients.iter_mut().filter(|c| c.joining) {
            c.joining = false;
            let name = c.name.clone().unwrap_or_default();
            match self.world.player_by_name(&name) {
                Some(p) => {
                    c.player = Some(p.id);
                    let snap = snapshot.get_or_insert_with(|| self.world.to_bytes()).clone();
                    c.conn.send(&ServerMsg::Welcome { player: p.id, world: self.cfg.name.clone(), snapshot: snap });
                    self.log.push(format!("{name} entre dans le monde"));
                }
                None => {
                    c.conn.send(&ServerMsg::Refused("le monde est plein".into()));
                    c.gone = true;
                }
            }
        }
        if let Some(r) = &self.responder {
            r.poll(&self.info());
        }
        if self.last_save.elapsed() >= SAVE_EVERY {
            self.save();
        }
    }

    pub fn save(&mut self) {
        self.last_save = Instant::now();
        if let Some(path) = &self.cfg.save {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let tmp = path.with_extension("tmp");
            let r = std::fs::write(&tmp, self.world.to_bytes()).and_then(|_| std::fs::rename(&tmp, path));
            if let Err(e) = r {
                self.log.push(format!("sauvegarde impossible : {e}"));
            }
        }
    }

    /// Run until `stop` is set, at the normal game speed.
    pub fn run(mut self, stop: Arc<AtomicBool>, mut on_log: impl FnMut(&str)) {
        let dt = Duration::from_secs_f32(1.0 / TICKS_PER_SEC);
        let mut next = Instant::now();
        while !stop.load(Ordering::Relaxed) {
            self.tick();
            for l in self.log.drain(..) {
                on_log(&l);
            }
            next += dt;
            let now = Instant::now();
            if next > now {
                thread::sleep(next - now);
            } else if now - next > dt * 20 {
                next = now; // way behind: don't try to catch up
            }
        }
        self.save();
    }
}

/// A server running on a background thread; stops when dropped.
pub struct ServerHandle {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    pub port: u16,
}

impl ServerHandle {
    pub fn spawn(cfg: ServerConfig) -> io::Result<ServerHandle> {
        let server = Server::new(cfg)?;
        let port = server.port();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = thread::spawn(move || server.run(flag, |_| {}));
        Ok(ServerHandle { stop, thread: Some(thread), port })
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::Connecting;

    /// A real client joins a server with bots, replays frames and stays in
    /// sync; a second client joins later from a snapshot.
    #[test]
    fn clients_follow_the_server() {
        let cfg = ServerConfig { name: "test".into(), port: 0, seed: 3, bots: 2, level: Level::Hard, save: None };
        let mut server = Server::new(cfg).unwrap();
        let port = server.port();

        let connect = |name: &str| {
            let c = Connecting::start(&format!("127.0.0.1:{port}"));
            let stream = loop {
                if let Some(r) = c.poll() {
                    break r.unwrap();
                }
                thread::sleep(Duration::from_millis(5));
            };
            let mut conn: Conn<ServerMsg> = Conn::new(stream).unwrap();
            conn.send(&ClientMsg::Hello { version: PROTOCOL_VERSION, name: name.into() });
            conn
        };
        struct Cl {
            conn: Conn<ServerMsg>,
            world: Option<World>,
            me: u8,
        }
        let mut clients = vec![Cl { conn: connect("alice"), world: None, me: 0 }];
        for t in 0..1200 {
            if t == 500 {
                clients.push(Cl { conn: connect("bob"), world: None, me: 0 });
            }
            server.tick();
            thread::sleep(Duration::from_millis(1));
            for c in &mut clients {
                for ev in c.conn.poll() {
                    match ev {
                        NetEvent::Msg(ServerMsg::Welcome { player, snapshot, .. }) => {
                            c.world = Some(World::from_bytes(&snapshot).unwrap());
                            c.me = player;
                        }
                        NetEvent::Msg(ServerMsg::Frame { tick, cmds }) => {
                            let w = c.world.as_mut().expect("welcome first");
                            assert_eq!(w.tick, tick);
                            w.step(&cmds);
                            if t == 300 && c.me == 2 {
                                let f = w.factories.iter().find(|f| f.owner == c.me).unwrap().id;
                                c.conn.send(&ClientMsg::Cmd(Command::SetQuota { factory: f, quota: 3 }));
                            }
                        }
                        NetEvent::Msg(other) => panic!("unexpected {other:?}"),
                        NetEvent::Closed(e) => panic!("closed: {e}"),
                    }
                }
            }
        }
        // Drain what is still in flight.
        thread::sleep(Duration::from_millis(50));
        for c in &mut clients {
            for ev in c.conn.poll() {
                if let NetEvent::Msg(ServerMsg::Frame { cmds, .. }) = ev {
                    c.world.as_mut().unwrap().step(&cmds);
                }
            }
            let w = c.world.as_ref().unwrap();
            assert_eq!(w.tick, server.world.tick);
            assert_eq!(w.state_hash(), server.world.state_hash());
        }
        assert_eq!(server.world.players.len(), 4);
        assert_eq!(clients[0].me, 2);
        assert_eq!(clients[1].me, 3);
    }
}

#[cfg(test)]
mod threaded {
    use super::*;
    use crate::net::Connecting;

    #[test]
    fn threaded_server_streams_frames() {
        let cfg = ServerConfig { name: "t".into(), port: 0, seed: 1, bots: 2, level: Level::Normal, save: None };
        let h = ServerHandle::spawn(cfg).unwrap();
        let c = Connecting::start(&format!("127.0.0.1:{}", h.port));
        let stream = loop {
            if let Some(r) = c.poll() {
                break r.unwrap();
            }
            thread::sleep(Duration::from_millis(5));
        };
        let mut conn: Conn<ServerMsg> = Conn::new(stream).unwrap();
        conn.send(&ClientMsg::Hello { version: PROTOCOL_VERSION, name: "zed".into() });
        let t0 = Instant::now();
        let (mut welcome, mut frames) = (None, 0);
        while t0.elapsed() < Duration::from_secs(2) {
            for ev in conn.poll() {
                match ev {
                    NetEvent::Msg(ServerMsg::Welcome { snapshot, .. }) => welcome = Some(World::from_bytes(&snapshot).unwrap().tick),
                    NetEvent::Msg(ServerMsg::Frame { tick, .. }) => {
                        if frames == 0 {
                            println!("first frame tick {tick}, welcome tick {welcome:?}");
                        }
                        frames += 1;
                    }
                    NetEvent::Msg(m) => println!("other {m:?}"),
                    NetEvent::Closed(e) => panic!("closed {e}"),
                }
            }
            thread::sleep(Duration::from_millis(10));
        }
        println!("welcome {welcome:?}, frames {frames}");
        assert!(welcome.is_some() && frames > 10);
    }
}
