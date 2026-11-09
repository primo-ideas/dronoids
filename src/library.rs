//! The player's program library. On the desktop: `.dasm` files in
//! `./programs`; in the browser: the page's local storage. Built-in
//! programs are copied in on first launch so they can be edited like any
//! other.

use std::collections::BTreeMap;

use dronoids::programs::{self, BUILTINS, TEMPLATE};
use dronoids::vm;

pub struct Entry {
    pub name: String,
    pub source: String,
}

pub struct Library {
    pub entries: Vec<Entry>,
}

pub fn sanitize(name: &str) -> String {
    let s: String = name
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .take(24)
        .collect();
    if s.is_empty() { "programme".into() } else { s }
}

pub fn name_char_ok(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

#[cfg(not(target_arch = "wasm32"))]
mod disk {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;

    fn path(name: &str) -> PathBuf {
        PathBuf::from("programs").join(format!("{name}.dasm"))
    }

    pub fn read_all() -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        if let Ok(rd) = fs::read_dir("programs") {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()) != Some("dasm") {
                    continue;
                }
                let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else { continue };
                if let Ok(src) = fs::read_to_string(&p) {
                    out.insert(super::sanitize(stem), src.replace("\r\n", "\n"));
                }
            }
        }
        out
    }

    pub fn write(name: &str, source: &str, _all: &BTreeMap<String, String>) -> Result<(), String> {
        let _ = fs::create_dir_all("programs");
        fs::write(path(name), source).map_err(|e| format!("sauvegarde impossible : {e}"))
    }

    pub fn rename(from: &str, to: &str) {
        let _ = fs::rename(path(from), path(to));
    }
}

#[cfg(target_arch = "wasm32")]
mod disk {
    use std::collections::BTreeMap;

    use crate::platform;

    pub fn read_all() -> BTreeMap<String, String> {
        platform::load("programs").and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn write(name: &str, source: &str, all: &BTreeMap<String, String>) -> Result<(), String> {
        let mut all = all.clone();
        all.insert(name.to_string(), source.to_string());
        let json = serde_json::to_string(&all).map_err(|e| e.to_string())?;
        platform::store("programs", &json);
        Ok(())
    }

    pub fn rename(_from: &str, _to: &str) {}
}

impl Library {
    pub fn load() -> Library {
        let mut all = disk::read_all();
        for (name, source) in all.clone() {
            let clean = programs::reworded(&source);
            if clean != source {
                all.insert(name.clone(), clean.clone());
                let _ = disk::write(&name, &clean, &all);
            }
        }
        for b in BUILTINS {
            match all.get(b.name) {
                // Written for an older version of the language: keep it aside.
                Some(old) if vm::assemble(b.name, old).is_err() => {
                    let old = old.clone();
                    disk::rename(b.name, &format!("{}-v1", b.name));
                    all.insert(format!("{}-v1", b.name), old);
                    all.insert(b.name.to_string(), b.source.to_string());
                    let _ = disk::write(b.name, b.source, &all);
                }
                Some(_) => {}
                None => {
                    all.insert(b.name.to_string(), b.source.to_string());
                    let _ = disk::write(b.name, b.source, &all);
                }
            }
        }
        let mut lib = Library { entries: all.into_iter().map(|(name, source)| Entry { name, source }).collect() };
        lib.sort();
        lib
    }

    fn sort(&mut self) {
        // Built-ins first, in their canonical order, then alphabetical.
        let rank = |n: &str| BUILTINS.iter().position(|b| b.name == n).unwrap_or(usize::MAX);
        self.entries.sort_by(|a, b| rank(&a.name).cmp(&rank(&b.name)).then(a.name.cmp(&b.name)));
    }

    fn as_map(&self) -> BTreeMap<String, String> {
        self.entries.iter().map(|e| (e.name.clone(), e.source.clone())).collect()
    }

    pub fn get(&self, name: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.name == name)
    }

    pub fn source_or_builtin(&self, name: &str) -> Option<String> {
        self.get(name)
            .map(|e| e.source.clone())
            .or_else(|| BUILTINS.iter().find(|b| b.name == name).map(|b| b.source.to_string()))
    }

    pub fn save(&mut self, name: &str, source: &str) -> Result<String, String> {
        let name = sanitize(name);
        disk::write(&name, source, &self.as_map())?;
        match self.entries.iter_mut().find(|e| e.name == name) {
            Some(e) => e.source = source.to_string(),
            None => {
                self.entries.push(Entry { name: name.clone(), source: source.to_string() });
                self.sort();
            }
        }
        Ok(name)
    }

    pub fn unique_name(&self, base: &str) -> String {
        let base = sanitize(base);
        if self.get(&base).is_none() {
            return base;
        }
        for i in 2.. {
            let n = format!("{base}{i}");
            if self.get(&n).is_none() {
                return n;
            }
        }
        unreachable!()
    }

    pub fn create(&mut self) -> Result<String, String> {
        let name = self.unique_name("prog");
        self.save(&name, TEMPLATE)
    }

    pub fn is_builtin(name: &str) -> bool {
        BUILTINS.iter().any(|b| b.name == name)
    }

    pub fn builtin_source(name: &str) -> Option<&'static str> {
        BUILTINS.iter().find(|b| b.name == name).map(|b| b.source)
    }
}
