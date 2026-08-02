use std::process::Command;

#[test]
fn sample_matches_flake8_async_semantics() {
    let out = Command::new(env!("CARGO_BIN_EXE_ty-async"))
        .arg("--async200-blocking-calls=stripe.*->alt(),ctx.storage.*->(),*.load_data->(),markdownify->()")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/sample.py"))
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    let positions: Vec<&str> = stdout
        .lines()
        .map(|l| l.split_once(".py:").unwrap().1.split_once(": ").unwrap().0)
        .collect();
    // awaited, noqa'd, nested-sync-def, lambda, and sync-fn calls must be absent
    assert_eq!(positions, ["4:5", "6:5", "7:9", "8:9", "11:5", "15:9"], "{stdout}");
    assert_eq!(out.status.code(), Some(1));
}
