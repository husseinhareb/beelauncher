mod app;
mod app_index;
mod cli;
mod config;
mod exec;
mod frequency;
mod input;
mod search;
mod ui;
mod wayland;

use std::process;

fn main() {
    env_logger::init();

    let opts = cli::Options::from_env();

    let cfg = match config::Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("beelauncher: config error: {e}");
            process::exit(1);
        }
    };

    if let Err(e) = wayland::run(cfg, opts) {
        eprintln!("beelauncher: {e}");
        process::exit(1);
    }
}
