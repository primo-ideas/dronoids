//! Minimal LAN networking: one TCP connection, newline-delimited JSON.
//! A background thread reads; the game thread polls a channel and writes.

use std::io::{self, BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::lockstep::Turn;

pub const PROTOCOL_VERSION: u32 = 1;
pub const DEFAULT_PORT: u16 = 7777;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Msg {
    /// Client -> host, right after connecting.
    Join { version: u32, name: String },
    /// Host -> client: everything needed to build the same world.
    Start { seed: u64, names: Vec<String>, your_id: u8 },
    Turn(Turn),
    Bye,
}

pub enum NetEvent {
    Msg(Msg),
    Closed(String),
}

pub struct Peer {
    writer: TcpStream,
    rx: Receiver<NetEvent>,
    closed: bool,
}

impl Peer {
    pub fn new(stream: TcpStream) -> io::Result<Peer> {
        stream.set_nodelay(true)?;
        let reader = stream.try_clone()?;
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut lines = BufReader::new(reader);
            let mut line = String::new();
            loop {
                line.clear();
                match lines.read_line(&mut line) {
                    Ok(0) => {
                        let _ = tx.send(NetEvent::Closed("connexion fermée".into()));
                        return;
                    }
                    Ok(_) => match serde_json::from_str::<Msg>(line.trim_end()) {
                        Ok(m) => {
                            if tx.send(NetEvent::Msg(m)).is_err() {
                                return;
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(NetEvent::Closed(format!("message invalide : {e}")));
                            return;
                        }
                    },
                    Err(e) => {
                        let _ = tx.send(NetEvent::Closed(e.to_string()));
                        return;
                    }
                }
            }
        });
        Ok(Peer { writer: stream, rx, closed: false })
    }

    pub fn send(&mut self, msg: &Msg) {
        if self.closed {
            return;
        }
        let mut s = serde_json::to_string(msg).expect("serializable");
        s.push('\n');
        if self.writer.write_all(s.as_bytes()).is_err() {
            self.closed = true;
        }
    }

    /// Next pending event, leaving the others queued. Use this while a
    /// message may hand the connection over to someone else (lobby -> game).
    pub fn poll_one(&mut self) -> Option<NetEvent> {
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

    pub fn poll(&mut self) -> Vec<NetEvent> {
        let mut out = vec![];
        while let Some(e) = self.poll_one() {
            out.push(e);
        }
        out
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        let _ = self.writer.shutdown(std::net::Shutdown::Both);
    }
}

/// Waits for one client in the background.
pub struct Listening {
    rx: Receiver<io::Result<TcpStream>>,
    pub port: u16,
}

impl Listening {
    pub fn start(port: u16) -> io::Result<Listening> {
        let listener = TcpListener::bind(("0.0.0.0", port))?;
        let port = listener.local_addr()?.port();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let r = listener.accept().map(|(s, _)| s);
            let _ = tx.send(r);
        });
        Ok(Listening { rx, port })
    }

    pub fn poll(&self) -> Option<io::Result<TcpStream>> {
        self.rx.try_recv().ok()
    }
}

/// Connects in the background so the UI keeps running.
pub struct Connecting {
    rx: Receiver<io::Result<TcpStream>>,
}

impl Connecting {
    pub fn start(addr: &str) -> Connecting {
        let addr = if addr.contains(':') { addr.to_string() } else { format!("{addr}:{DEFAULT_PORT}") };
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

/// Best guess of this machine's LAN address (no packet is sent).
pub fn local_ip() -> Option<String> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.168.0.1:9").ok()?;
    Some(s.local_addr().ok()?.ip().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_roundtrip() {
        let l = Listening::start(0).unwrap();
        let c = Connecting::start(&format!("127.0.0.1:{}", l.port));
        let mut server = None;
        let mut client = None;
        for _ in 0..200 {
            if server.is_none() {
                server = l.poll().map(|r| Peer::new(r.unwrap()).unwrap());
            }
            if client.is_none() {
                client = c.poll().map(|r| Peer::new(r.unwrap()).unwrap());
            }
            if server.is_some() && client.is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let (mut server, mut client) = (server.unwrap(), client.unwrap());
        client.send(&Msg::Join { version: PROTOCOL_VERSION, name: "x\ny".into() });
        let mut got = None;
        for _ in 0..200 {
            if let Some(NetEvent::Msg(m)) = server.poll().into_iter().next() {
                got = Some(m);
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        match got {
            Some(Msg::Join { name, .. }) => assert_eq!(name, "x\ny"),
            other => panic!("unexpected {other:?}"),
        }
    }
}
