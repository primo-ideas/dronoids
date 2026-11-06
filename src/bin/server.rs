//! Dedicated, headless world server.
//!
//! ```text
//! god-server [--name nebuleuse] [--port 7777] [--bots 4] [--level easy|normal|hard]
//!            [--seed N] [--save worlds/nebuleuse.world]
//! ```
//! The world is saved every minute (and loaded again on the next start).

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use game_of_drones::ai::Level;
use game_of_drones::net::{local_ip, DEFAULT_PORT};
use game_of_drones::server::{Server, ServerConfig};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let get = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("god-server [--name NOM] [--port 7777] [--bots 4] [--level easy|normal|hard] [--seed N] [--save FICHIER]");
        return;
    }
    let name = get("--name").unwrap_or_else(|| "nebuleuse".into());
    let level = match get("--level").as_deref() {
        Some("easy") => Level::Easy,
        Some("hard") => Level::Hard,
        _ => Level::Normal,
    };
    let cfg = ServerConfig {
        port: get("--port").and_then(|p| p.parse().ok()).unwrap_or(DEFAULT_PORT),
        seed: get("--seed").and_then(|s| s.parse().ok()).unwrap_or_else(|| {
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1)
        }),
        bots: get("--bots").and_then(|b| b.parse().ok()).unwrap_or(4),
        level,
        save: Some(get("--save").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("worlds").join(format!("{name}.world")))),
        name,
    };
    let server = match Server::new(cfg.clone()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("impossible de démarrer : {e}");
            std::process::exit(1);
        }
    };
    println!(
        "monde « {} » sur {}:{} ({} IA, {}) — sauvegarde : {}",
        cfg.name,
        local_ip().unwrap_or_else(|| "0.0.0.0".into()),
        server.port(),
        cfg.bots,
        level.label(),
        cfg.save.as_ref().map(|p| p.display().to_string()).unwrap_or_default()
    );
    for l in &server.log {
        println!("{l}");
    }
    server.run(Arc::new(AtomicBool::new(false)), |l| println!("{l}"));
}
