use std::process::{Command, Output};

const ASYNC_PATTERNS: &str = "--async200-blocking-calls=stripe.*->alt(),stripe.bar2.*->second(),ctx.storage.*->(),*.load_data->(),markdownify->()";
const HATCHES: &str = "--type-hatches=no-any,no-getattr,no-object";

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ty-async"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(args)
        .output()
        .unwrap()
}

fn expected(name: &str) -> String {
    std::fs::read_to_string(format!("{}/tests/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

/// tests/sample.expected is flake8's own output for the same invocation
/// (regenerate: flake8 --select=ASYNC200 <patterns> tests/sample.py).
/// Covers: awaited exemption, noqa handling, nested def/lambda context,
/// deep attribute chains, call-result bases, same-position duplicates,
/// comprehensions, elif tests, pattern precedence.
#[test]
fn async200_matches_flake8() {
    let out = run(&[ASYNC_PATTERNS, "tests/sample.py"]);
    assert_eq!(String::from_utf8(out.stdout).unwrap(), expected("sample.expected"));
    assert_eq!(out.status.code(), Some(1));
}

/// tests/hatches.expected holds the same finding set as
/// lint/type_escape_hatches.py (verified set-equal; ordering here is ours).
/// Covers: every typing-module Any spelling, string annotations, all
/// parameter kinds, AnnAssign, return annotations, decorated defs,
/// if/elif statement anchoring, lint-ignore suppression (same-line,
/// multi-line signature, multi-code), getattr/hasattr vs attribute access.
#[test]
fn type_hatches_match_python_tool() {
    let out = run(&[HATCHES, "tests/hatches.py"]);
    assert_eq!(String::from_utf8(out.stdout).unwrap(), expected("hatches.expected"));
    assert_eq!(out.status.code(), Some(1));
}

/// Both modes in one invocation: output is the sorted union.
#[test]
fn combined_modes() {
    let out = run(&[ASYNC_PATTERNS, HATCHES, "tests/sample.py", "tests/hatches.py"]);
    let mut want: Vec<String> = expected("sample.expected")
        .lines()
        .chain(expected("hatches.expected").lines())
        .map(String::from)
        .collect();
    // the binary sorts by (path, line, col) numerically, not lexically
    let key = |s: &str| {
        let mut it = s.splitn(4, ':');
        let path = it.next().unwrap_or("").to_string();
        let line: usize = it.next().unwrap_or("0").parse().unwrap_or(0);
        let col: usize = it.next().unwrap_or("0").parse().unwrap_or(0);
        (path, line, col, it.next().unwrap_or("").to_string())
    };
    want.sort_by_key(|s| key(s));
    let got: Vec<String> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(String::from)
        .collect();
    assert_eq!(got, want);
}

#[test]
fn clean_file_exits_zero() {
    let out = run(&[ASYNC_PATTERNS, HATCHES, "src"]);
    assert_eq!(out.status.code(), Some(0), "src/ has no .py files");
    assert!(out.stdout.is_empty());
}

#[test]
fn unknown_hatch_rule_is_an_error() {
    let out = run(&["--type-hatches=no-such-rule", "tests/hatches.py"]);
    assert_eq!(out.status.code(), Some(2));
}

/// Covers: multi-hop chains, same-class method resolution, cross-file
/// globally-unique resolution, ambiguity refusal, cycles, callables passed
/// to executors (not calls), async-to-async boundaries, and noqa.
#[test]
fn transitive_chains() {
    let out = run(&[
        "--async200-blocking-calls=stripe.*->alt()",
        "--async200-transitive",
        "tests/transitive",
    ]);
    assert_eq!(String::from_utf8(out.stdout).unwrap(), expected("transitive.expected"));
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn transitive_requires_patterns() {
    let out = run(&["--async200-transitive", "tests/transitive"]);
    assert_eq!(out.status.code(), Some(2));
}
