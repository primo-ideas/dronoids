//! Dedicated, headless world server.
//!
//! ```text
//! dronoids-server [--name primo] [--bind 0.0.0.0] [--port 8082] [--bots 6]
//!            [--level easy|normal|hard] [--seed N] [--save worlds/primo.world]
//!            [--site-secret FILE]
//! ```
//! `--site-secret` names the file holding the secret shared with the site's
//! account service: players signed in there are then recognized.
//! Clients connect over WebSocket (`ws://host:port/ws`). The world is saved
//! every minute and on exit, and loaded again on the next start.

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    use std::path::PathBuf;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    use dronoids::ai::Level;
    use dronoids::net::DEFAULT_PORT;
    use dronoids::server::{Server, ServerConfig};

    let args: Vec<String> = std::env::args().skip(1).collect();
    let get = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "dronoids-server [--name NOM] [--bind 0.0.0.0] [--port {DEFAULT_PORT}] [--bots 6] [--level easy|normal|hard] [--seed N] [--save FICHIER] [--site-secret FICHIER]"
        );
        return;
    }
    let name = get("--name").unwrap_or_else(|| "primo".into());
    let level = match get("--level").as_deref() {
        Some("easy") => Level::Easy,
        Some("hard") => Level::Hard,
        _ => Level::Normal,
    };
    let site_secret = get("--site-secret").and_then(|path| match std::fs::read_to_string(&path) {
        Ok(s) if s.trim().len() >= 16 => Some(s.trim().as_bytes().to_vec()),
        Ok(_) => {
            eprintln!("secret du site trop court dans {path} : les comptes du site seront refusés");
            None
        }
        Err(e) => {
            eprintln!("secret du site illisible ({path} : {e}) : les comptes du site seront refusés");
            None
        }
    });
    let cfg = ServerConfig {
        bind: get("--bind").unwrap_or_else(|| "0.0.0.0".into()),
        port: get("--port").and_then(|p| p.parse().ok()).unwrap_or(DEFAULT_PORT),
        seed: get("--seed").and_then(|s| s.parse().ok()).unwrap_or_else(|| {
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1)
        }),
        bots: get("--bots").and_then(|b| b.parse().ok()).unwrap_or(6),
        level,
        save: Some(get("--save").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("worlds").join(format!("{name}.world")))),
        name,
        site_secret,
    };
    let mut server = match Server::new(cfg.clone()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("impossible de démarrer : {e}");
            std::process::exit(1);
        }
    };
    println!(
        "monde « {} » sur ws://{}:{}/ws ({} colonies IA, {}) — sauvegarde : {}",
        cfg.name,
        cfg.bind,
        server.port(),
        cfg.bots,
        level.label(),
        cfg.save.as_ref().map(|p| p.display().to_string()).unwrap_or_default()
    );
    for l in server.log.drain(..) {
        println!("{l}");
    }
    server.run(Arc::new(AtomicBool::new(false)), |l| println!("{l}"));
}

#[cfg(target_arch = "wasm32")]
fn main() {}
