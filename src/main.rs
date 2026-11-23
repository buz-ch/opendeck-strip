use std::ffi::OsString;

use opendeck_strip::{cli, config, logging, plugin, status};

fn main() {
    let argv: Vec<OsString> = std::env::args_os().collect();
    if let Some(args) = plugin::args_from_argv(argv) {
        logging::init(&status::status_path());
        log_config_warnings();
        match plugin::run(args) {
            Ok(()) => std::process::exit(0),
            Err(err) => {
                log::error!("{err:#}");
                std::process::exit(1);
            }
        }
    }

    logging::init(&status::status_path());
    log_config_warnings();
    match cli::run() {
        Ok(code) => std::process::exit(code),
        Err(err) => {
            eprintln!("error: {err:#}");
            std::process::exit(1);
        }
    }
}

/// The command loads the config itself; this only reports what was wrong with it.
fn log_config_warnings() {
    for warning in config::Config::load().warnings {
        log::warn!("{warning}");
    }
}
