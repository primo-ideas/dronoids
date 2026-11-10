//! What differs between the desktop and the browser: where little things
//! are stored, which server to talk to, and where randomness comes from.

use serde::{Deserialize, Serialize};

/// Remembered between sessions.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Settings {
    pub name: String,
    /// Secret proving that `name` is ours on the server.
    pub token: String,
    /// Empty: the default server.
    pub server: String,
}

impl Settings {
    pub fn load() -> Settings {
        let mut s: Settings = load("settings").and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        if s.token.len() < 16 {
            s.token = random_token();
        }
        if s.name.trim().is_empty() {
            s.name = default_name();
        }
        s.save();
        s
    }

    pub fn save(&self) {
        if let Ok(t) = serde_json::to_string(self) {
            store("settings", &t);
        }
    }

    pub fn server_url(&self) -> String {
        if self.server.trim().is_empty() { default_server() } else { self.server.trim().to_string() }
    }
}

fn random_token() -> String {
    (0..4).map(|_| format!("{:08x}", random_u32())).collect()
}

pub fn seed() -> u64 {
    (random_u32() as u64) << 32 | random_u32() as u64
}

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use std::hash::{BuildHasher, Hasher};
    use std::path::PathBuf;

    fn path(key: &str) -> PathBuf {
        PathBuf::from("save").join(format!("{key}.json"))
    }

    pub fn load(key: &str) -> Option<String> {
        std::fs::read_to_string(path(key)).ok()
    }

    pub fn store(key: &str, value: &str) {
        let p = path(key);
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, value);
    }

    /// `DRONOIDS_SERVER` (formerly `GOD_SERVER`) overrides the local default.
    pub fn default_server() -> String {
        std::env::var("DRONOIDS_SERVER")
            .or_else(|_| std::env::var("GOD_SERVER"))
            .unwrap_or_else(|_| "ws://127.0.0.1:8082/ws".into())
    }

    pub fn default_name() -> String {
        std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap_or_else(|_| "Pilot".into())
    }

    pub fn random_u32() -> u32 {
        // RandomState is seeded by the operating system.
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        h.finish() as u32
    }

    pub fn is_web() -> bool {
        false
    }

    /// Desktop keyboards are always there.
    pub fn keyboard(_open: bool) {}

    /// The desktop client has no site account.
    pub fn account() -> Option<(String, String)> {
        None
    }

    pub fn open_site() {}

    pub fn typed() -> String {
        String::new()
    }
}

#[cfg(target_arch = "wasm32")]
mod imp {
    #[link(wasm_import_module = "env")]
    unsafe extern "C" {
        fn god_storage_len(kptr: *const u8, klen: usize) -> i32;
        fn god_storage_read(kptr: *const u8, klen: usize, ptr: *mut u8, cap: usize);
        fn god_storage_write(kptr: *const u8, klen: usize, vptr: *const u8, vlen: usize);
        fn god_default_server(ptr: *mut u8, cap: usize) -> i32;
        fn god_random() -> u32;
        fn god_keyboard(open: u32);
        fn god_account(ptr: *mut u8, cap: usize) -> i32;
        fn god_open_site();
        fn god_typed_len() -> i32;
        fn god_typed_take(ptr: *mut u8, len: usize);
    }

    pub fn load(key: &str) -> Option<String> {
        let len = unsafe { god_storage_len(key.as_ptr(), key.len()) };
        if len < 0 {
            return None;
        }
        let mut buf = vec![0u8; len as usize];
        unsafe { god_storage_read(key.as_ptr(), key.len(), buf.as_mut_ptr(), buf.len()) };
        String::from_utf8(buf).ok()
    }

    pub fn store(key: &str, value: &str) {
        unsafe { god_storage_write(key.as_ptr(), key.len(), value.as_ptr(), value.len()) };
    }

    /// Same host and directory as the page: `wss://site/dronoids/ws`.
    pub fn default_server() -> String {
        let mut buf = vec![0u8; 512];
        let n = unsafe { god_default_server(buf.as_mut_ptr(), buf.len()) };
        buf.truncate(n.max(0) as usize);
        String::from_utf8(buf).unwrap_or_default()
    }

    pub fn default_name() -> String {
        format!("Pilot{}", random_u32() % 1000)
    }

    pub fn random_u32() -> u32 {
        unsafe { god_random() }
    }

    pub fn is_web() -> bool {
        true
    }

    /// The phone's on-screen keyboard: `true` brings it up when the finger
    /// that touched a text field is lifted (browsers only open it then).
    pub fn keyboard(open: bool) {
        unsafe { god_keyboard(open as u32) };
    }

    /// The nickname and ticket of the account signed in on the site, if any.
    pub fn account() -> Option<(String, String)> {
        let mut buf = vec![0u8; 512];
        let n = unsafe { god_account(buf.as_mut_ptr(), buf.len()) };
        buf.truncate(n.max(0) as usize);
        let text = String::from_utf8(buf).ok()?;
        let (nickname, ticket) = text.split_once('\n')?;
        (!nickname.is_empty() && !ticket.is_empty()).then(|| (nickname.to_string(), ticket.to_string()))
    }

    /// The site's account page (leaves the game).
    pub fn open_site() {
        unsafe { god_open_site() };
    }

    /// What was typed on the on-screen keyboard since last call; U+0008 is
    /// a backspace, `\n` is Enter.
    pub fn typed() -> String {
        let len = unsafe { god_typed_len() };
        if len <= 0 {
            return String::new();
        }
        let mut buf = vec![0u8; len as usize];
        unsafe { god_typed_take(buf.as_mut_ptr(), buf.len()) };
        String::from_utf8(buf).unwrap_or_default()
    }
}

pub use imp::{account, default_name, default_server, is_web, keyboard, load, open_site, random_u32, store, typed};
