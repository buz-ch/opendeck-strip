//! Golden parity with the plugin's renderer (spec §9.1): every example of
//! `tests/zjse/examples.toml`, rendered from its document through the config
//! keys, must equal `tests/zjse/expected/<name>.txt` byte for byte.
use std::path::PathBuf;

use opendeck_strip::config::Config;
use opendeck_strip::zjse::{Options, examples, render};

fn expected_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/zjse/expected")
}

#[test]
fn the_manifest_lists_the_29_examples_once_each() {
    let all = examples::load().expect("examples.toml");
    assert_eq!(all.len(), 29);
    let mut names: Vec<_> = all.iter().map(|e| e.name.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), all.len());
}

#[test]
fn every_example_matches_the_plugin_output() {
    let all = examples::load().expect("examples.toml");
    let mut failed = Vec::new();
    for e in &all {
        let path = expected_dir().join(format!("{}.txt", e.name));
        let want = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("{}: {err}", path.display()));
        // `Config::default()` with the example's `zjse_*` overrides.
        let config: Config = e.config().expect("example config");
        let o = Options::from_config(&config);
        let doc = e.document().expect("scenario document");
        let got = render(&doc, &o, examples::NOW_MS);
        if got.as_deref() != Some(want.as_str()) {
            failed.push(format!("{}\n  want: {want:?}\n   got: {got:?}", e.name));
        }
    }
    assert!(
        failed.is_empty(),
        "{} of {} examples differ:\n{}",
        failed.len(),
        all.len(),
        failed.join("\n")
    );
}
