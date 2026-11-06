//! The player's program library: `.dasm` files in `./programs`.
//! Built-in programs are written there on first launch so they can be
//! edited like any other.

use std::fs;
use std::path::PathBuf;

use game_of_drones::programs::{BUILTINS, TEMPLATE};
use game_of_drones::vm;

pub struct Entry {
    pub name: String,
    pub source: String,
}

pub struct Library {
    dir: PathBuf,
    pub entries: Vec<Entry>,
    pub error: Option<String>,
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

impl Library {
    pub fn load() -> Library {
        let dir = PathBuf::from("programs");
        let mut lib = Library { dir, entries: vec![], error: None };
        if let Err(e) = fs::create_dir_all(&lib.dir) {
            lib.error = Some(format!("dossier programs/ : {e}"));
        }
        for b in BUILTINS {
            let p = lib.path(b.name);
            match fs::read_to_string(&p) {
                // Written for an older version of the language: keep it aside.
                Ok(old) if vm::assemble(b.name, &old).is_err() => {
                    let _ = fs::rename(&p, lib.path(&format!("{}-v1", b.name)));
                    let _ = fs::write(&p, b.source);
                }
                Ok(_) => {}
                Err(_) => {
                    let _ = fs::write(&p, b.source);
                }
            }
        }
        lib.reload();
        // Even if the disk is read-only, the built-ins must be available.
        for b in BUILTINS {
            if lib.get(b.name).is_none() {
                lib.entries.push(Entry { name: b.name.into(), source: b.source.into() });
            }
        }
        lib.sort();
        lib
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.dasm"))
    }

    fn sort(&mut self) {
        // Built-ins first, in their canonical order, then alphabetical.
        let rank = |n: &str| BUILTINS.iter().position(|b| b.name == n).unwrap_or(usize::MAX);
        self.entries.sort_by(|a, b| rank(&a.name).cmp(&rank(&b.name)).then(a.name.cmp(&b.name)));
    }

    pub fn reload(&mut self) {
        self.entries.clear();
        if let Ok(rd) = fs::read_dir(&self.dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()) != Some("dasm") {
                    continue;
                }
                let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else { continue };
                if let Ok(src) = fs::read_to_string(&p) {
                    self.entries.push(Entry { name: sanitize(stem), source: src.replace("\r\n", "\n") });
                }
            }
        }
        self.sort();
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
        fs::write(self.path(&name), source).map_err(|e| format!("sauvegarde impossible : {e}"))?;
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
