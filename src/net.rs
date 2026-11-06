//! Networking: length-prefixed bincode messages over TCP (one reader and
//! one writer thread per connection, so the game loop never blocks), plus
//! UDP broadcast to discover the worlds hosted on the local network.

use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::sim::Command;

pub const PROTOCOL_VERSION: u32 = 2;
pub const DEFAULT_PORT: u16 = 7777;
pub const DISCOVERY_PORT: u16 = 7778;
const MAX_MESSAGE: usize = 256 << 20;
const PING: &[u8] = b"GAME-OF-DRONES?";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ClientMsg {
    Hello { version: u32, name: String },
    Cmd(Command),
    /// Hash of the client's world when it reached `tick`.
    Hash { tick: u64, hash: u64 },
    Bye,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ServerMsg {
    /// You are `player`; here is the world (`World::to_bytes`).
    Welcome { player: u8, world: String, snapshot: Vec<u8> },
    /// Commands to apply when stepping from `tick` to `tick + 1`.
    Frame { tick: u64, cmds: Vec<(u8, Command)> },
    /// Your world diverged: start again from this one.
    Resync { snapshot: Vec<u8> },
    Refused(String),
}

pub enum NetEvent<T> {
    Msg(T),
    Closed(String),
}

/// One TCP connection receiving `In` messages.
pub struct Conn<In> {
    out: Sender<Vec<u8>>,
    rx: Receiver<NetEvent<In>>,
    closed: bool,
}

impl<In: DeserializeOwned + Send + 'static> Conn<In> {
    pub fn new(stream: TcpStream) -> io::Result<Conn<In>> {
        stream.set_nodelay(true)?;
        stream.set_nonblocking(false)?;
        let mut reader = stream.try_clone()?;
        let mut writer = stream.try_clone()?;
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let close = |tx: &Sender<NetEvent<In>>, why: String| {
                let _ = tx.send(NetEvent::Closed(why));
            };
            loop {
                let mut len = [0u8; 4];
                if let Err(e) = reader.read_exact(&mut len) {
                    let why = if e.kind() == io::ErrorKind::UnexpectedEof { "connexion fermée".into() } else { e.to_string() };
                    return close(&tx, why);
                }
                let len = u32::from_le_bytes(len) as usize;
                if len > MAX_MESSAGE {
                    return close(&tx, "message trop gros".into());
                }
                let mut buf = vec![0u8; len];
                if let Err(e) = reader.read_exact(&mut buf) {
                    return close(&tx, e.to_string());
                }
                match bincode::deserialize::<In>(&buf) {
                    Ok(m) => {
                        if tx.send(NetEvent::Msg(m)).is_err() {
                            return;
                        }
                    }
                    Err(e) => return close(&tx, format!("message invalide : {e}")),
                }
            }
        });
        let (out, out_rx) = mpsc::channel::<Vec<u8>>();
        // The writer flushes everything queued, then closes the socket once
        // the `Conn` is dropped (so a last "Refused" still gets through).
        thread::spawn(move || {
            for buf in out_rx {
                let len = (buf.len() as u32).to_le_bytes();
                if writer.write_all(&len).is_err() || writer.write_all(&buf).is_err() {
                    break;
                }
            }
            let _ = writer.flush();
            let _ = writer.shutdown(std::net::Shutdown::Both);
        });
        drop(stream);
        Ok(Conn { out, rx, closed: false })
    }

    pub fn send<Out: Serialize>(&mut self, msg: &Out) {
        if self.closed {
            return;
        }
        let buf = bincode::serialize(msg).expect("serializable");
        if self.out.send(buf).is_err() {
            self.closed = true;
        }
    }

    /// Next pending event, leaving the others queued (useful while a
    /// message may hand the connection over, e.g. lobby -> game).
    pub fn poll_one(&mut self) -> Option<NetEvent<In>> {
        match self.rx.try_recv() {
            Ok(e) => {
                if matches!(e, NetEvent::Closed(_)) {
                    self.closed = true;
                }
                Some(e)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                if self.closed {
                    None
                } else {
                    self.closed = true;
                    Some(NetEvent::Closed("connexion perdue".into()))
                }
            }
        }
    }

    pub fn poll(&mut self) -> Vec<NetEvent<In>> {
        let mut v = vec![];
        while let Some(e) = self.poll_one() {
            v.push(e);
        }
        v
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }
}

/// Connects in the background so the UI keeps running.
pub struct Connecting {
    rx: Receiver<io::Result<TcpStream>>,
}

pub fn with_default_port(addr: &str) -> String {
    if addr.contains(':') { addr.to_string() } else { format!("{addr}:{DEFAULT_PORT}") }
}

impl Connecting {
    pub fn start(addr: &str) -> Connecting {
        let addr = with_default_port(addr);
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let r = (|| {
                let target = addr
                    .to_socket_addrs()?
                    .next()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "adresse introuvable"))?;
                TcpStream::connect_timeout(&target, Duration::from_secs(5))
            })();
            let _ = tx.send(r);
        });
        Connecting { rx }
    }

    pub fn poll(&self) -> Option<io::Result<TcpStream>> {
        self.rx.try_recv().ok()
    }
}

// ---------------------------------------------------------------------------
// LAN discovery
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorldInfo {
    pub name: String,
    pub port: u16,
    pub players: u32,
    pub online: u32,
    pub bots: u32,
    pub tick: u64,
    pub version: u32,
}

/// Server side: answers discovery pings.
pub struct Responder {
    sock: UdpSocket,
}

impl Responder {
    pub fn bind() -> io::Result<Responder> {
        let sock = UdpSocket::bind(("0.0.0.0", DISCOVERY_PORT))?;
        sock.set_nonblocking(true)?;
        Ok(Responder { sock })
    }

    pub fn poll(&self, info: &WorldInfo) {
        let mut buf = [0u8; 64];
        while let Ok((n, from)) = self.sock.recv_from(&mut buf) {
            if &buf[..n] == PING
                && let Ok(reply) = serde_json::to_vec(info) {
                    let _ = self.sock.send_to(&reply, from);
                }
        }
    }
}

#[derive(Clone, Debug)]
pub struct Found {
    pub addr: String,
    pub info: WorldInfo,
    pub seen: f64,
}

/// Client side: broadcasts pings and collects answers.
pub struct Browser {
    sock: UdpSocket,
    pub found: Vec<Found>,
}

impl Browser {
    pub fn new() -> io::Result<Browser> {
        let sock = UdpSocket::bind(("0.0.0.0", 0))?;
        sock.set_broadcast(true)?;
        sock.set_nonblocking(true)?;
        Ok(Browser { sock, found: vec![] })
    }

    pub fn ping(&self) {
        for target in ["255.255.255.255", "127.0.0.1"] {
            let _ = self.sock.send_to(PING, (target, DISCOVERY_PORT));
        }
    }

    pub fn poll(&mut self, now: f64) {
        let mut buf = [0u8; 2048];
        while let Ok((n, from)) = self.sock.recv_from(&mut buf) {
            let Ok(info) = serde_json::from_slice::<WorldInfo>(&buf[..n]) else { continue };
            let ip = match from {
                SocketAddr::V4(a) => a.ip().to_string(),
                SocketAddr::V6(a) => a.ip().to_string(),
            };
            let addr = format!("{ip}:{}", info.port);
            match self.found.iter_mut().find(|f| f.addr == addr || (f.info.name == info.name && f.info.port == info.port)) {
                Some(f) => {
                    // Prefer a real LAN address over loopback for display.
                    if f.addr.starts_with("127.") && !ip.starts_with("127.") {
                        f.addr = addr;
                    }
                    f.info = info;
                    f.seen = now;
                }
                None => self.found.push(Found { addr, info, seen: now }),
            }
        }
        self.found.retain(|f| now - f.seen < 6.0);
        self.found.sort_by(|a, b| a.info.name.cmp(&b.info.name));
    }
}

/// Best guess of this machine's LAN address (no packet is sent).
pub fn local_ip() -> Option<String> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.168.0.1:9").ok()?;
    Some(s.local_addr().ok()?.ip().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn framed_roundtrip() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let c = Connecting::start(&format!("127.0.0.1:{port}"));
        let (s, _) = l.accept().unwrap();
        let mut server: Conn<ClientMsg> = Conn::new(s).unwrap();
        let mut stream = None;
        for _ in 0..200 {
            if let Some(r) = c.poll() {
                stream = Some(r.unwrap());
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let mut client: Conn<ServerMsg> = Conn::new(stream.unwrap()).unwrap();
        client.send(&ClientMsg::Hello { version: PROTOCOL_VERSION, name: "x\ny".into() });
        server.send(&ServerMsg::Frame { tick: 3, cmds: vec![(1, Command::ToggleFactory { factory: 9 })] });
        let mut got = (false, false);
        for _ in 0..200 {
            if let Some(NetEvent::Msg(ClientMsg::Hello { name, .. })) = server.poll_one() {
                assert_eq!(name, "x\ny");
                got.0 = true;
            }
            if let Some(NetEvent::Msg(ServerMsg::Frame { tick, cmds })) = client.poll_one() {
                assert_eq!((tick, cmds.len()), (3, 1));
                got.1 = true;
            }
            if got == (true, true) {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(got, (true, true));
    }
}
