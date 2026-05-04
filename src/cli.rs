//! Command-line argument parsing.
//!
//! Current grammar:
//!   beelauncher [--show <item>[,<item>...]]
//!
//! --show accepts a comma-separated list of items.  Only "icons" is recognised
//! now; unknown items are warned and ignored so old scripts don't break when
//! new items are added.

use std::process;

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub show_icons: bool,
}

impl Options {
    pub fn from_env() -> Self {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let mut opts = Self::default();
        let mut i = 0;

        while i < args.len() {
            match args[i].as_str() {
                "--show" => {
                    let val = args.get(i + 1).map(String::as_str).unwrap_or_else(|| {
                        eprintln!("beelauncher: --show requires an argument");
                        process::exit(1);
                    });
                    for item in val.split(',') {
                        match item.trim() {
                            "icons" => opts.show_icons = true,
                            other => eprintln!("beelauncher: unknown --show item '{other}' (ignored)"),
                        }
                    }
                    i += 2;
                }
                "--help" | "-h" => {
                    println!("{}", HELP);
                    process::exit(0);
                }
                other => {
                    eprintln!("beelauncher: unknown argument '{other}'");
                    process::exit(1);
                }
            }
        }

        opts
    }
}

const HELP: &str = "\
beelauncher — Wayland application launcher

USAGE:
    beelauncher [OPTIONS]

OPTIONS:
    --show <items>   Comma-separated list of extra UI items to display.
                     Recognised items: icons
    --help           Show this message and exit

EXAMPLES:
    beelauncher
    beelauncher --show icons
";
