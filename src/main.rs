//! Fast ASYNC200 linter (flake8-async's user-configured blocking-call check),
//! built on ruff's Python parser.
//!
//! Replicates flake8-async semantics: inside `async def` bodies (nested `def`
//! and `lambda` reset the context), every call that is not directly awaited
//! has `ast.unparse(node.func)` fnmatch-ed against the configured patterns.

use rayon::prelude::*;
use ruff_python_ast::visitor::{walk_expr, Visitor};
use ruff_python_ast::{Expr, ExprCall, Stmt};
use ruff_text_size::{Ranged, TextSize};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

struct Diagnostic {
    offset: TextSize,
    pattern_idx: usize,
}

/// fnmatch-style glob: `*` matches any run of characters (including `.`),
/// `?` matches one byte. Anchored at both ends, like Python's fnmatch.
// ponytail: `[seq]` classes are treated literally; add if a config ever uses them.
fn glob_match(pat: &[u8], text: &[u8]) -> bool {
    let (mut p, mut t) = (0usize, 0usize);
    let (mut star_p, mut star_t) = (usize::MAX, 0usize);
    while t < text.len() {
        if p < pat.len() && (pat[p] == b'?' || pat[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pat.len() && pat[p] == b'*' {
            star_p = p;
            star_t = t;
            p += 1;
        } else if star_p != usize::MAX {
            p = star_p + 1;
            star_t += 1;
            t = star_t;
        } else {
            return false;
        }
    }
    while p < pat.len() && pat[p] == b'*' {
        p += 1;
    }
    p == pat.len()
}

/// Approximation of `ast.unparse(call.func)`, good enough for fnmatch.
// ponytail: non-name bases render as placeholders ("(...)", "[...]") — a `*`
// in the pattern matches them just like it matches ast.unparse's full text.
fn unparse(expr: &Expr, out: &mut String) {
    match expr {
        Expr::Name(n) => out.push_str(n.id.as_str()),
        Expr::Attribute(a) => {
            unparse(&a.value, out);
            out.push('.');
            out.push_str(a.attr.as_str());
        }
        Expr::Call(c) => {
            unparse(&c.func, out);
            out.push_str("(...)");
        }
        Expr::Subscript(s) => {
            unparse(&s.value, out);
            out.push_str("[...]");
        }
        _ => out.push('\u{1}'), // never matches a literal pattern char, only `*`/`?`
    }
}

struct Checker<'a> {
    patterns: &'a [(String, String)],
    in_async: bool,
    diagnostics: Vec<Diagnostic>,
}

impl Checker<'_> {
    fn check_call(&mut self, call: &ExprCall) {
        if !self.in_async {
            return;
        }
        let mut name = String::new();
        unparse(&call.func, &mut name);
        for (i, (pat, _)) in self.patterns.iter().enumerate() {
            if glob_match(pat.as_bytes(), name.as_bytes()) {
                self.diagnostics.push(Diagnostic {
                    offset: call.range().start(),
                    pattern_idx: i,
                });
                break;
            }
        }
    }

    /// Visit a call's children without checking the call itself (for `await f()`).
    fn walk_call_children(&mut self, call: &ExprCall) {
        self.visit_expr(&call.func);
        for arg in &*call.arguments.args {
            self.visit_expr(arg);
        }
        for kw in &*call.arguments.keywords {
            self.visit_expr(&kw.value);
        }
    }
}

impl<'a> Visitor<'a> for Checker<'_> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::FunctionDef(f) => {
                let prev = self.in_async;
                self.in_async = f.is_async;
                ruff_python_ast::visitor::walk_stmt(self, stmt);
                self.in_async = prev;
            }
            // ruff's walk_stmt visits each elif test twice (directly and via
            // walk_elif_else_clause), which would double-report; walk If ourselves.
            Stmt::If(if_stmt) => {
                self.visit_expr(&if_stmt.test);
                self.visit_body(&if_stmt.body);
                for clause in &if_stmt.elif_else_clauses {
                    if let Some(test) = &clause.test {
                        self.visit_expr(test);
                    }
                    self.visit_body(&clause.body);
                }
            }
            _ => ruff_python_ast::visitor::walk_stmt(self, stmt),
        }
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::Lambda(_) => {
                let prev = self.in_async;
                self.in_async = false;
                walk_expr(self, expr);
                self.in_async = prev;
            }
            Expr::Await(aw) => {
                if let Expr::Call(call) = &*aw.value {
                    self.walk_call_children(call);
                } else {
                    walk_expr(self, expr);
                }
            }
            Expr::Call(call) => {
                self.check_call(call);
                walk_expr(self, expr);
            }
            _ => walk_expr(self, expr),
        }
    }
}

/// Does a `# noqa` comment on this line suppress ASYNC200?
// ponytail: substring scan instead of flake8's full regex; equivalent on sane code.
fn noqa_suppresses(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    for (i, b) in lower.bytes().enumerate() {
        if b != b'#' {
            continue;
        }
        let rest = lower[i + 1..].trim_start_matches([' ', '#']);
        if let Some(after) = rest.strip_prefix("noqa") {
            match after.trim_start().chars().next() {
                None => return true,
                Some(':') => {
                    let codes = &after.trim_start()[1..];
                    let codes = codes.split('#').next().unwrap_or("");
                    if codes
                        .split([',', ' ', '\t', ';'])
                        .any(|c| c.trim() == "async200")
                    {
                        return true;
                    }
                }
                Some(c) if !c.is_ascii_alphanumeric() => return true,
                _ => {}
            }
        }
    }
    false
}

fn check_file(path: &Path, patterns: &[(String, String)]) -> Vec<(PathBuf, usize, usize, usize)> {
    let Ok(source) = std::fs::read_to_string(path) else {
        eprintln!("warning: could not read {}", path.display());
        return Vec::new();
    };
    let parsed = match ruff_python_parser::parse_module(&source) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("warning: could not parse {}: {e}", path.display());
            return Vec::new();
        }
    };
    let mut checker = Checker {
        patterns,
        in_async: false,
        diagnostics: Vec::new(),
    };
    for stmt in &parsed.syntax().body {
        checker.visit_stmt(stmt);
    }
    if checker.diagnostics.is_empty() {
        return Vec::new();
    }

    let mut line_starts = vec![0usize];
    for (i, b) in source.bytes().enumerate() {
        if b == b'\n' {
            line_starts.push(i + 1);
        }
    }
    checker
        .diagnostics
        .into_iter()
        .filter_map(|d| {
            let off = usize::from(d.offset);
            let line = line_starts.partition_point(|&s| s <= off);
            let start = line_starts[line - 1];
            let end = line_starts.get(line).map_or(source.len(), |&e| e - 1);
            let line_text = &source[start..end];
            if noqa_suppresses(line_text) {
                None
            } else {
                Some((path.to_path_buf(), line, off - start + 1, d.pattern_idx))
            }
        })
        .collect()
}

fn excluded(rel: &Path, excludes: &[String]) -> bool {
    let rel_str = rel.to_string_lossy();
    let base = rel
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();
    excludes.iter().any(|pat| {
        *rel_str == **pat
            || rel_str.starts_with(&format!("{pat}/"))
            || glob_match(pat.as_bytes(), base.as_bytes())
            || glob_match(pat.as_bytes(), rel_str.as_bytes())
    })
}

fn collect_files(path: &Path, excludes: &[String], out: &mut Vec<PathBuf>) {
    if excluded(path, excludes) {
        return;
    }
    let Ok(meta) = path.symlink_metadata() else {
        return;
    };
    if meta.is_dir() {
        let Ok(entries) = std::fs::read_dir(path) else {
            return;
        };
        for entry in entries.flatten() {
            collect_files(&entry.path(), excludes, out);
        }
    } else if meta.is_file() && path.extension().is_some_and(|e| e == "py") {
        out.push(path.to_path_buf());
    }
}

fn take_value(arg: &str, args: &mut impl Iterator<Item = String>) -> String {
    arg.split_once('=')
        .map(|(_, v)| v.to_string())
        .or_else(|| args.next())
        .unwrap_or_default()
}

fn main() -> ExitCode {
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut excludes: Vec<String> = Vec::new();
    let mut patterns: Vec<(String, String)> = Vec::new();

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--exclude" || arg.starts_with("--exclude=") {
            let v = take_value(&arg, &mut args);
            excludes.extend(
                v.split(',')
                    .map(|s| s.trim().trim_end_matches('/').to_string()),
            );
        } else if arg == "--async200-blocking-calls" || arg.starts_with("--async200-blocking-calls=")
        {
            let v = take_value(&arg, &mut args);
            for entry in v.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                match entry.split_once("->") {
                    Some((k, r)) => patterns.push((k.trim().to_string(), r.trim().to_string())),
                    None => {
                        eprintln!("error: missing '->' in blocking-calls entry {entry:?}");
                        return ExitCode::from(2);
                    }
                }
            }
        } else if arg == "--ignore" || arg.starts_with("--ignore=") {
            take_value(&arg, &mut args); // only ASYNC200 is implemented
        } else if arg == "--asyncio" {
            // no-op: only affects message wording for codes we don't implement
        } else if arg.starts_with('-') {
            eprintln!("note: ignoring unsupported flag {arg}");
        } else {
            paths.push(PathBuf::from(arg));
        }
    }
    if paths.is_empty() {
        paths.push(PathBuf::from("."));
    }

    let mut files = Vec::new();
    for p in &paths {
        collect_files(p, &excludes, &mut files);
    }

    let mut results: Vec<(PathBuf, usize, usize, usize)> = files
        .par_iter()
        .flat_map_iter(|f| check_file(f, &patterns))
        .collect();
    results.sort();

    for (path, line, col, idx) in &results {
        let (pat, repl) = &patterns[*idx];
        println!(
            "{}:{line}:{col}: ASYNC200 User-configured blocking sync call {pat} in async function, consider replacing with {repl}.",
            path.display()
        );
    }
    if results.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
