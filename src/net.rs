//! Networking: bincode messages over WebSocket, so browsers can play.
//!
//! Native builds (desktop client, server) use `tungstenite` with one thread
//! per connection; the browser build goes through the page's WebSocket via
//! a tiny JavaScript plugin (`web/god_net.js`). Both expose the same `Conn`.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::sim::Command;

pub const PROTOCOL_VERSION: u32 = 3;
pub const DEFAULT_PORT: u16 = 8082;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ClientMsg {
    /// `token` proves ownership of `name` across sessions.
    Hello { version: u32, name: String, token: String },
    Cmd(Command),
    /// Hash of the client's world when it reached `tick`.
    Hash { tick: u64, hash: u64 },
    /// The client fell too far behind (e.g. a hidden browser tab).
    WantSnapshot,
    Bye,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ServerMsg {
    /// You are `player`; here is the world (`World::to_bytes`).
    Welcome { player: u8, world: String, snapshot: Vec<u8> },
    /// Commands to apply when stepping from `tick` to `tick + 1`.
    Frame { tick: u64, cmds: Vec<(u8, Command)> },
    /// Start again from this world (divergence, or on request).
    Resync { snapshot: Vec<u8> },
    /// How many people are connected right now.
    Online { players: Vec<u8> },
    Refused(String),
}

pub enum NetEvent<T> {
    Msg(T),
    Closed(String),
}

/// `ws://host:port/ws` from a bare `host` or `host:port`.
pub fn normalize_url(addr: &str) -> String {
    let a = addr.trim();
    if a.starts_with("ws://") || a.starts_with("wss://") {
        return a.to_string();
    }
    let host = if a.contains(':') { a.to_string() } else { format!("{a}:{DEFAULT_PORT}") };
    format!("ws://{host}/ws")
}

fn decode<In: DeserializeOwned>(bytes: &[u8]) -> Result<In, String> {
    bincode::deserialize(bytes).map_err(|e| format!("message invalide : {e}"))
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::*;
#[cfg(target_arch = "wasm32")]
pub use web::*;

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::io;
    use std::marker::PhantomData;
    use std::net::{TcpStream, ToSocketAddrs};
    use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
    use std::thread;
    use std::time::Duration;

    use serde::de::DeserializeOwned;
    use serde::Serialize;
    use tungstenite::{Message, WebSocket};

    use super::{decode, NetEvent};

    /// One WebSocket connection receiving `In` messages.
    pub struct Conn<In> {
        out: Sender<Vec<u8>>,
        rx: Receiver<NetEvent<In>>,
        closed: bool,
        _in: PhantomData<In>,
    }

    /// Reads (with a short timeout) and writes on one thread until either
    /// side goes away.
    fn pump<In: DeserializeOwned>(mut ws: WebSocket<TcpStream>, tx: Sender<NetEvent<In>>, out: Receiver<Vec<u8>>) {
        let _ = ws.get_mut().set_read_timeout(Some(Duration::from_millis(15)));
        let _ = ws.get_mut().set_nodelay(true);
        let close = |why: String| {
            let _ = tx.send(NetEvent::Closed(why));
        };
        loop {
            loop {
                match out.try_recv() {
                    Ok(buf) => {
                        if let Err(e) = ws.send(Message::binary(buf)) {
                            return close(e.to_string());
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        // Our side is done: say goodbye properly.
                        let _ = ws.close(None);
                        let _ = ws.flush();
                        return;
                    }
                }
            }
            match ws.read() {
                Ok(Message::Binary(data)) => match decode::<In>(&data) {
                    Ok(m) => {
                        if tx.send(NetEvent::Msg(m)).is_err() {
                            return;
                        }
                    }
                    Err(e) => return close(e),
                },
                Ok(Message::Close(_)) => return close("connexion fermée".into()),
                Ok(_) => {}
                Err(tungstenite::Error::Io(e)) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {
                    // Pings get answered on the next write.
                    let _ = ws.flush();
                }
                Err(tungstenite::Error::ConnectionClosed) | Err(tungstenite::Error::AlreadyClosed) => {
                    return close("connexion fermée".into());
                }
                Err(e) => return close(e.to_string()),
            }
        }
    }

    impl<In: DeserializeOwned + Send + 'static> Conn<In> {
        fn spawn(ws: WebSocket<TcpStream>) -> Conn<In> {
            let (tx, rx) = mpsc::channel();
            let (out, out_rx) = mpsc::channel();
            thread::spawn(move || pump(ws, tx, out_rx));
            Conn { out, rx, closed: false, _in: PhantomData }
        }

        /// Server side: perform the WebSocket handshake on an accepted socket.
        pub fn accept(stream: TcpStream) -> Result<Conn<In>, String> {
            stream.set_nonblocking(false).map_err(|e| e.to_string())?;
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let ws = tungstenite::accept(stream).map_err(|e| e.to_string())?;
            Ok(Conn::spawn(ws))
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
        /// message may hand the connection over, e.g. menu -> game).
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
    pub struct Connecting<In> {
        rx: Receiver<Result<Conn<In>, String>>,
    }

    impl<In: DeserializeOwned + Send + 'static> Connecting<In> {
        pub fn start(url: &str) -> Connecting<In> {
            let url = super::normalize_url(url);
            let (tx, rx) = mpsc::channel();
            thread::spawn(move || {
                let r = (|| -> Result<Conn<In>, String> {
                    if url.starts_with("wss://") {
                        return Err("wss:// n'est disponible que dans le navigateur (utilisez ws://)".into());
                    }
                    let hostport = url.trim_start_matches("ws://").split('/').next().unwrap_or_default().to_string();
                    let addr = hostport
                        .to_socket_addrs()
                        .map_err(|e| e.to_string())?
                        .next()
                        .ok_or_else(|| "adresse introuvable".to_string())?;
                    let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5)).map_err(|e| e.to_string())?;
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                    let (ws, _) = tungstenite::client(url.as_str(), stream).map_err(|e| e.to_string())?;
                    Ok(Conn::spawn(ws))
                })();
                let _ = tx.send(r);
            });
            Connecting { rx }
        }

        pub fn poll(&mut self) -> Option<Result<Conn<In>, String>> {
            self.rx.try_recv().ok()
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::marker::PhantomData;

    use serde::de::DeserializeOwned;
    use serde::Serialize;

    use super::{decode, NetEvent};

    #[link(wasm_import_module = "env")]
    unsafe extern "C" {
        fn god_ws_open(ptr: *const u8, len: usize) -> i32;
        fn god_ws_state(id: i32) -> i32;
        fn god_ws_send(id: i32, ptr: *const u8, len: usize);
        fn god_ws_peek(id: i32) -> i32;
        fn god_ws_take(id: i32, ptr: *mut u8, len: usize);
        fn god_ws_close(id: i32);
    }

    const OPEN: i32 = 1;
    const CLOSED: i32 = 2;

    /// The page's WebSocket, seen through `web/god_net.js`.
    pub struct Conn<In> {
        id: i32,
        closed: bool,
        _in: PhantomData<In>,
    }

    impl<In: DeserializeOwned> Conn<In> {
        pub fn send<Out: Serialize>(&mut self, msg: &Out) {
            if self.closed {
                return;
            }
            let buf = bincode::serialize(msg).expect("serializable");
            unsafe { god_ws_send(self.id, buf.as_ptr(), buf.len()) };
        }

        pub fn poll_one(&mut self) -> Option<NetEvent<In>> {
            if self.closed {
                return None;
            }
            let len = unsafe { god_ws_peek(self.id) };
            if len >= 0 {
                let mut buf = vec![0u8; len as usize];
                unsafe { god_ws_take(self.id, buf.as_mut_ptr(), buf.len()) };
                return Some(match decode::<In>(&buf) {
                    Ok(m) => NetEvent::Msg(m),
                    Err(e) => {
                        self.closed = true;
                        NetEvent::Closed(e)
                    }
                });
            }
            if unsafe { god_ws_state(self.id) } == CLOSED {
                self.closed = true;
                return Some(NetEvent::Closed("connexion fermée".into()));
            }
            None
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

    impl<In> Drop for Conn<In> {
        fn drop(&mut self) {
            unsafe { god_ws_close(self.id) };
        }
    }

    pub struct Connecting<In> {
        id: Option<i32>,
        _in: PhantomData<In>,
    }

    impl<In: DeserializeOwned> Connecting<In> {
        pub fn start(url: &str) -> Connecting<In> {
            let url = super::normalize_url(url);
            let id = unsafe { god_ws_open(url.as_ptr(), url.len()) };
            Connecting { id: Some(id), _in: PhantomData }
        }

        pub fn poll(&mut self) -> Option<Result<Conn<In>, String>> {
            let id = self.id?;
            match unsafe { god_ws_state(id) } {
                OPEN => {
                    self.id = None;
                    Some(Ok(Conn { id, closed: false, _in: PhantomData }))
                }
                CLOSED => {
                    self.id = None;
                    unsafe { god_ws_close(id) };
                    Some(Err("serveur injoignable".into()))
                }
                _ => None,
            }
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn websocket_roundtrip() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (s, _) = l.accept().unwrap();
            Conn::<ClientMsg>::accept(s).unwrap()
        });
        let mut c = Connecting::<ServerMsg>::start(&format!("127.0.0.1:{port}"));
        let mut client = loop {
            if let Some(r) = c.poll() {
                break r.unwrap();
            }
            thread::sleep(Duration::from_millis(5));
        };
        let mut server = server.join().unwrap();
        client.send(&ClientMsg::Hello { version: PROTOCOL_VERSION, name: "x\ny".into(), token: "t".into() });
        server.send(&ServerMsg::Frame { tick: 3, cmds: vec![(1, Command::ToggleFactory { factory: 9 })] });
        let mut got = (false, false);
        for _ in 0..300 {
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
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(got, (true, true));
        drop(client);
        let mut closed = false;
        for _ in 0..300 {
            if let Some(NetEvent::Closed(_)) = server.poll_one() {
                closed = true;
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        assert!(closed, "server notices the client leaving");
    }

    #[test]
    fn urls() {
        assert_eq!(normalize_url("example.org"), "ws://example.org:8082/ws");
        assert_eq!(normalize_url("10.0.0.2:9000"), "ws://10.0.0.2:9000/ws");
        assert_eq!(normalize_url("wss://a.b/drones/ws"), "wss://a.b/drones/ws");
    }
}
