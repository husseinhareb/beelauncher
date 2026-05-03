mod app;
mod app_index;
mod config;
mod exec;
mod input;
mod search;
mod ui;
mod wayland;

use std::process;

fn main() {
    env_logger::init();

    let cfg = match config::Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("beelauncher: config error: {e}");
            process::exit(1);
        }
    };

    if let Err(e) = wayland::run(cfg) {
        eprintln!("beelauncher: {e}");
        process::exit(1);
    }
}
