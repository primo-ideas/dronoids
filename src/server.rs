//! World server. It owns the authoritative world, runs the computer
//! colonies, and each tick broadcasts the ordered list of commands; clients
//! replay them on their own copy (deterministic simulation), so only
//! commands travel — never the thousands of drones. A newcomer receives a
//! snapshot, then the frames. Clients report hashes; a client that drifts
//! (or falls behind, like a hidden browser tab) gets a fresh snapshot.
//!
//! Names are protected by a secret token the client keeps: the first
//! connection with a name claims it.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::ai::{Bot, Level};
use crate::net::{ClientMsg, Conn, NetEvent, ServerMsg, PROTOCOL_VERSION};
use crate::rules::TICKS_PER_SEC;
use crate::sim::{Command, World};

pub const HASH_EVERY: u64 = 50;
const SAVE_EVERY: Duration = Duration::from_secs(30);
const PRESENCE_EVERY: u64 = 20;

pub const BOT_NAMES: &[&str] = &[
    "Orion", "Vega", "Lyra", "Rigel", "Deneb", "Sirius", "Altaïr", "Antarès", "Castor", "Pollux", "Capella", "Mira",
];

#[derive(Clone, Debug)]
pub struct ServerConfig {
    pub name: String,
    /// Address to listen on ("127.0.0.1" behind a reverse proxy).
    pub bind: String,
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
    port: u16,
    incoming: Receiver<Conn<ClientMsg>>,
    clients: Vec<Client>,
    pending: Vec<(u8, Command)>,
    hashes: BTreeMap<u64, u64>,
    /// name -> secret token.
    accounts: HashMap<String, String>,
    online: BTreeSet<u8>,
    last_save: Instant,
    pub log: Vec<String>,
}

pub fn bot_name(i: usize) -> String {
    let base = BOT_NAMES[i % BOT_NAMES.len()];
    if i < BOT_NAMES.len() { format!("IA {base}") } else { format!("IA {base} {}", i / BOT_NAMES.len() + 1) }
}

fn accounts_path(save: &Option<PathBuf>) -> Option<PathBuf> {
    save.as_ref().map(|p| p.with_extension("accounts.json"))
}

impl Server {
    pub fn new(cfg: ServerConfig) -> io::Result<Server> {
        let listener = TcpListener::bind((cfg.bind.as_str(), cfg.port))?;
        let port = listener.local_addr()?.port();
        // Handshakes happen on their own threads: a slow or bogus client
        // never stalls the world.
        let (tx, incoming) = mpsc::channel();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let tx = tx.clone();
                thread::spawn(move || {
                    if let Ok(conn) = Conn::<ClientMsg>::accept(stream) {
                        let _ = tx.send(conn);
                    }
                });
            }
        });

        let mut log = vec![];
        let loaded = cfg.save.as_ref().filter(|p| p.exists()).and_then(|p| match std::fs::read(p) {
            Ok(bytes) => match World::from_save(&bytes) {
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
        let accounts = accounts_path(&cfg.save)
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Ok(Server {
            cfg,
            world,
            bots,
            port,
            incoming,
            clients: vec![],
            pending: vec![],
            hashes: BTreeMap::new(),
            accounts,
            online: BTreeSet::new(),
            last_save: Instant::now(),
            log,
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    fn handle_messages(&mut self) {
        while let Ok(conn) = self.incoming.try_recv() {
            self.clients.push(Client { conn, name: None, player: None, joining: false, gone: false });
        }
        for ci in 0..self.clients.len() {
            while let Some(ev) = self.clients[ci].conn.poll_one() {
                match ev {
                    NetEvent::Msg(ClientMsg::Hello { version, name, token }) => {
                        let name: String = name.trim().chars().filter(|c| !c.is_control()).take(20).collect();
                        let connected = self.clients.iter().any(|c| c.name.as_deref() == Some(name.as_str()) && !c.gone);
                        let is_bot = self.world.player_by_name(&name).is_some_and(|p| p.bot);
                        let owned_by_other = self.accounts.get(&name).is_some_and(|t| *t != token);
                        let c = &mut self.clients[ci];
                        let refuse = if version != PROTOCOL_VERSION {
                            Some("version du jeu différente : rechargez la page".to_string())
                        } else if name.is_empty() || token.len() < 8 {
                            Some("nom invalide".into())
                        } else if is_bot || owned_by_other {
                            Some(format!("le nom « {name} » appartient déjà à quelqu'un"))
                        } else if connected {
                            Some(format!("« {name} » est déjà connecté"))
                        } else {
                            None
                        };
                        match refuse {
                            Some(why) => {
                                c.conn.send(&ServerMsg::Refused(why));
                                c.gone = true;
                            }
                            None => {
                                self.accounts.entry(name.clone()).or_insert(token);
                                c.name = Some(name.clone());
                                c.joining = true;
                                self.pending.push((0, Command::Join { name, bot: false }));
                            }
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
                            && mine != hash
                        {
                            let c = &mut self.clients[ci];
                            self.log.push(format!("{} désynchronisé au tick {tick}", c.name.clone().unwrap_or_default()));
                            c.conn.send(&ServerMsg::Resync { snapshot: self.world.to_bytes() });
                        }
                    }
                    NetEvent::Msg(ClientMsg::WantSnapshot) => {
                        if self.clients[ci].player.is_some() {
                            let snap = self.world.to_bytes();
                            self.clients[ci].conn.send(&ServerMsg::Resync { snapshot: snap });
                        }
                    }
                    NetEvent::Msg(ClientMsg::Bye) | NetEvent::Closed(_) => {
                        self.clients[ci].gone = true;
                        break;
                    }
                }
            }
        }
        for c in self.clients.iter().filter(|c| c.gone && c.player.is_some()) {
            self.log.push(format!("{} se déconnecte", c.name.clone().unwrap_or_default()));
        }
        self.clients.retain(|c| !c.gone);
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
        let online: BTreeSet<u8> = self.clients.iter().filter_map(|c| c.player).collect();
        if online != self.online || self.world.tick.is_multiple_of(PRESENCE_EVERY) {
            self.online = online;
            let msg = ServerMsg::Online { players: self.online.iter().copied().collect() };
            for c in self.clients.iter_mut().filter(|c| c.player.is_some()) {
                c.conn.send(&msg);
            }
        }
        if self.last_save.elapsed() >= SAVE_EVERY {
            self.save();
        }
    }

    pub fn save(&mut self) {
        self.last_save = Instant::now();
        let Some(path) = &self.cfg.save else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let write = |path: &PathBuf, bytes: &[u8]| {
            let tmp = path.with_extension("tmp");
            std::fs::write(&tmp, bytes).and_then(|_| std::fs::rename(&tmp, path))
        };
        if let Err(e) = write(path, &self.world.to_save()) {
            self.log.push(format!("sauvegarde impossible : {e}"));
        }
        if let Some(ap) = accounts_path(&self.cfg.save)
            && let Ok(json) = serde_json::to_vec(&self.accounts)
        {
            let _ = write(&ap, &json);
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

    fn cfg(bots: usize) -> ServerConfig {
        ServerConfig { name: "test".into(), bind: "127.0.0.1".into(), port: 0, seed: 3, bots, level: Level::Hard, save: None }
    }

    fn connect(port: u16, name: &str, token: &str) -> Conn<ServerMsg> {
        let mut c = Connecting::<ServerMsg>::start(&format!("127.0.0.1:{port}"));
        let mut conn = loop {
            if let Some(r) = c.poll() {
                break r.unwrap();
            }
            thread::sleep(Duration::from_millis(5));
        };
        conn.send(&ClientMsg::Hello { version: PROTOCOL_VERSION, name: name.into(), token: token.into() });
        conn
    }

    /// Real clients join a server with bots, replay frames and stay in
    /// sync; a second client joins later from a snapshot.
    #[test]
    fn clients_follow_the_server() {
        let mut server = Server::new(cfg(2)).unwrap();
        let port = server.port();
        struct Cl {
            conn: Conn<ServerMsg>,
            world: Option<World>,
            me: u8,
        }
        let mut clients = vec![Cl { conn: connect(port, "alice", "secret-alice"), world: None, me: 0 }];
        for t in 0..1200 {
            if t == 500 {
                clients.push(Cl { conn: connect(port, "bob", "secret-bob"), world: None, me: 0 });
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
                            if w.is_landing(c.me) && w.players[c.me as usize].minerals >= crate::rules::FACTORY_COST {
                                let (hx, hy) = w.players[c.me as usize].home;
                                let m = crate::programs::builtin("mineur").unwrap();
                                c.conn.send(&ClientMsg::Cmd(Command::PlaceFactory {
                                    x: hx,
                                    y: hy,
                                    name: m.name.into(),
                                    source: m.source.into(),
                                }));
                            }
                        }
                        NetEvent::Msg(ServerMsg::Online { .. }) => {}
                        NetEvent::Msg(other) => panic!("unexpected {other:?}"),
                        NetEvent::Closed(e) => panic!("closed: {e}"),
                    }
                }
            }
        }
        thread::sleep(Duration::from_millis(80));
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
        assert_eq!((clients[0].me, clients[1].me), (2, 3));
        assert!(server.world.factory_count(2) >= 1 && server.world.factory_count(3) >= 1, "both landed");
    }

    #[test]
    fn names_are_protected_by_their_token() {
        let mut server = Server::new(cfg(0)).unwrap();
        let port = server.port();
        let mut first = connect(port, "zoe", "token-one-1234");
        let mut got_welcome = false;
        for _ in 0..200 {
            server.tick();
            thread::sleep(Duration::from_millis(2));
            if first.poll().iter().any(|e| matches!(e, NetEvent::Msg(ServerMsg::Welcome { .. }))) {
                got_welcome = true;
                break;
            }
        }
        assert!(got_welcome);
        drop(first);
        let mut thief = connect(port, "zoe", "another-token-99");
        let mut refused = false;
        for _ in 0..200 {
            server.tick();
            thread::sleep(Duration::from_millis(2));
            if thief.poll().iter().any(|e| matches!(e, NetEvent::Msg(ServerMsg::Refused(_)))) {
                refused = true;
                break;
            }
        }
        assert!(refused, "someone else's name must be refused");
    }

    #[test]
    fn threaded_server_streams_frames() {
        let h = ServerHandle::spawn(ServerConfig { bots: 2, ..cfg(0) }).unwrap();
        let mut conn = connect(h.port, "zed", "secret-zed-123");
        let t0 = Instant::now();
        let (mut welcome, mut frames) = (false, 0);
        while t0.elapsed() < Duration::from_secs(3) {
            for ev in conn.poll() {
                match ev {
                    NetEvent::Msg(ServerMsg::Welcome { .. }) => welcome = true,
                    NetEvent::Msg(ServerMsg::Frame { .. }) => frames += 1,
                    NetEvent::Msg(_) => {}
                    NetEvent::Closed(e) => panic!("closed {e}"),
                }
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(welcome && frames >= 8, "welcome {welcome}, frames {frames}");
    }
}
