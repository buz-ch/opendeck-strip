//! Prints the strip string of one named preview example:
//! `cargo run --example zjsb_preview -- <example-name>`
//! (`-- --config <name>` prints the example's keys as TOML lines instead).
use opendeck_strip::zjsb::{Mode, Options, examples, render};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (config, name) = match args.as_slice() {
        [flag, name] if flag == "--config" => (true, name),
        [name] => (false, name),
        _ => {
            eprintln!("usage: zjsb_preview [--config] <example-name>");
            std::process::exit(2);
        }
    };
    let run = || -> Result<String, String> {
        let all = examples::load()?;
        let e = all
            .iter()
            .find(|e| &e.name == name)
            .ok_or_else(|| format!("no example `{name}`"))?;
        if config {
            return Ok(e.overrides_toml().join("\n"));
        }
        let o = Options::from_config(&e.config()?);
        let text = render(&e.document()?, &o, examples::NOW_MS).unwrap_or_default();
        // One slot: shown as the first 200 px segment of an empty frame.
        let tail = if o.mode == Mode::Compact { "|||" } else { "" };
        Ok(format!("{text}{tail}"))
    };
    match run() {
        Ok(out) => print!("{out}"),
        Err(e) => {
            eprintln!("zjsb_preview: {e}");
            std::process::exit(1);
        }
    }
}
