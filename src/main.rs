use std::ffi::OsString;

use opendeck_strip::{cli, logging, plugin, status};

fn main() {
    let argv: Vec<OsString> = std::env::args_os().collect();
    if let Some(args) = plugin::args_from_argv(argv) {
        logging::init(&status::status_path());
        match plugin::run(args) {
            Ok(()) => std::process::exit(0),
            Err(err) => {
                log::error!("{err:#}");
                std::process::exit(1);
            }
        }
    }

    logging::init(&status::status_path());
    match cli::run() {
        Ok(code) => std::process::exit(code),
        Err(err) => {
            eprintln!("error: {err:#}");
            std::process::exit(1);
        }
    }
}
