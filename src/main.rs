//! Fast Python linter built on ruff's parser, implementing:
//!
//! - flake8-async's ASYNC200 check (user-configured blocking calls in async
//!   functions), via `--async200-blocking-calls='pat->replacement,...'`.
//! - The spot-platform "type escape hatch" rules, via
//!   `--type-hatches=no-any,no-getattr,no-object`:
//!     no-getattr : bare `getattr()` / `hasattr()` calls
//!     no-any     : `Any` anywhere inside an annotation
//!     no-object  : `object` anywhere inside an annotation
//!   suppressed per-site with `# lint-ignore: no-any,no-getattr` comments
//!   anywhere in the enclosing statement/signature span.
//!
//! ASYNC200 semantics match flake8-async: inside `async def` bodies (nested
//! `def` and `lambda` reset the context), every call that is not directly
//! awaited has `ast.unparse(node.func)` fnmatch-ed against the patterns.

use rayon::prelude::*;
use ruff_python_ast::visitor::{walk_expr, Visitor};
use ruff_python_ast::{Expr, ExprCall, Stmt, StmtAnnAssign, StmtFunctionDef};
use ruff_text_size::{Ranged, TextSize};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

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

const HATCH_CODES: [&str; 3] = ["no-any", "no-getattr", "no-object"];

#[derive(Default, Clone, Copy)]
struct Hatches {
    no_any: bool,
    no_getattr: bool,
    no_object: bool,
}

enum Diag {
    Async200 {
        offset: TextSize,
        pattern_idx: usize,
    },
    Hatch {
        code: &'static str,
        message: String,
        /// Offset of the offending node (column is computed on its own line).
        offset: TextSize,
        /// Span whose lines are searched for `# lint-ignore:` and whose first
        /// line is the reported line (enclosing statement / signature).
        span: (TextSize, TextSize),
    },
}

/// Names that identify the `typing` module when used as an attribute base
/// (`t`/`tp` cover `import typing as t`).
fn is_typing_any(expr: &Expr) -> bool {
    match expr {
        Expr::Name(n) => n.id.as_str() == "Any",
        Expr::Attribute(a) => {
            a.attr.as_str() == "Any"
                && matches!(&*a.value, Expr::Name(base)
                    if matches!(base.id.as_str(), "typing" | "typing_extensions" | "t" | "tp"))
        }
        _ => false,
    }
}

struct AnnScan<'a> {
    hatches: Hatches,
    span: (TextSize, TextSize),
    diags: &'a mut Vec<Diag>,
}

impl<'a> Visitor<'a> for AnnScan<'_> {
    fn visit_expr(&mut self, expr: &'a Expr) {
        if self.hatches.no_any && is_typing_any(expr) {
            self.diags.push(Diag::Hatch {
                code: "no-any",
                message: "`Any` in annotation".to_string(),
                offset: expr.range().start(),
                span: self.span,
            });
        }
        if self.hatches.no_object
            && matches!(expr, Expr::Name(n) if n.id.as_str() == "object")
        {
            self.diags.push(Diag::Hatch {
                code: "no-object",
                message: "`object` in annotation".to_string(),
                offset: expr.range().start(),
                span: self.span,
            });
        }
        walk_expr(self, expr);
    }
}

struct Checker<'a> {
    patterns: &'a [(String, String)],
    hatches: Hatches,
    in_async: bool,
    /// (anchor for the reported line, end) of the nearest enclosing statement.
    stmt_span: Option<(TextSize, TextSize)>,
    diags: Vec<Diag>,
}

impl Checker<'_> {
    /// `check_async200` is false for directly-awaited calls, which flake8-async
    /// exempts; no-getattr applies to them regardless (the Python tool flags
    /// every `getattr`/`hasattr` call).
    fn check_call(&mut self, call: &ExprCall, check_async200: bool) {
        if self.hatches.no_getattr {
            if let Expr::Name(n) = &*call.func {
                if matches!(n.id.as_str(), "getattr" | "hasattr") {
                    let span = self
                        .stmt_span
                        .unwrap_or((call.range().start(), call.range().end()));
                    self.diags.push(Diag::Hatch {
                        code: "no-getattr",
                        message: format!("`{}(...)` is banned", n.id.as_str()),
                        offset: call.range().start(),
                        span,
                    });
                }
            }
        }
        if !check_async200 || !self.in_async || self.patterns.is_empty() {
            return;
        }
        let mut name = String::new();
        unparse(&call.func, &mut name);
        for (i, (pat, _)) in self.patterns.iter().enumerate() {
            if glob_match(pat.as_bytes(), name.as_bytes()) {
                self.diags.push(Diag::Async200 {
                    offset: call.range().start(),
                    pattern_idx: i,
                });
                break;
            }
        }
    }

    /// Visit a call's children without re-visiting the call node itself.
    fn walk_call_children(&mut self, call: &ExprCall) {
        self.visit_expr(&call.func);
        for arg in &*call.arguments.args {
            self.visit_expr(arg);
        }
        for kw in &*call.arguments.keywords {
            self.visit_expr(&kw.value);
        }
    }

    fn scan_annotation(&mut self, ann: &Expr, span: (TextSize, TextSize)) {
        if !(self.hatches.no_any || self.hatches.no_object) {
            return;
        }
        AnnScan {
            hatches: self.hatches,
            span,
            diags: &mut self.diags,
        }
        .visit_expr(ann);
    }

    /// Signature span: `def` line through the line of the first body statement
    /// (mirrors the Python tool, wide enough that line-wrapping can't orphan
    /// the lint-ignore comment).
    fn function_hatches(&mut self, f: &StmtFunctionDef) {
        let start = f.name.range().start();
        let end = f
            .body
            .first()
            .map(|s| s.range().start())
            .unwrap_or_else(|| f.range().end());
        let span = (start, end);
        for param in f.parameters.iter() {
            if let Some(ann) = param.annotation() {
                self.scan_annotation(ann, span);
            }
        }
        if let Some(returns) = &f.returns {
            self.scan_annotation(returns, span);
        }
    }

    fn ann_assign_hatches(&mut self, a: &StmtAnnAssign) {
        self.scan_annotation(&a.annotation, (a.range().start(), a.range().end()));
    }
}

impl<'a> Visitor<'a> for Checker<'_> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        // Anchor the reported line at the `def`/`class` keyword line, like
        // Python's stmt.lineno (ruff's node range starts at the decorators).
        let anchor = match stmt {
            Stmt::FunctionDef(f) => f.name.range().start(),
            Stmt::ClassDef(c) => c.name.range().start(),
            _ => stmt.range().start(),
        };
        let prev = self.stmt_span.replace((anchor, stmt.range().end()));
        match stmt {
            Stmt::FunctionDef(f) => {
                self.function_hatches(f);
                let prev_async = self.in_async;
                self.in_async = f.is_async;
                ruff_python_ast::visitor::walk_stmt(self, stmt);
                self.in_async = prev_async;
            }
            Stmt::AnnAssign(a) => {
                self.ann_assign_hatches(a);
                ruff_python_ast::visitor::walk_stmt(self, stmt);
            }
            // ruff's walk_stmt visits each elif test twice (directly and via
            // walk_elif_else_clause), which would double-report; walk If ourselves.
            Stmt::If(if_stmt) => {
                self.visit_expr(&if_stmt.test);
                self.visit_body(&if_stmt.body);
                for clause in &if_stmt.elif_else_clauses {
                    if let Some(test) = &clause.test {
                        // Python's AST nests `elif` as an inner If statement, so
                        // its span runs from the `elif` keyword to the end of the
                        // remaining chain; mirror that for calls in elif tests.
                        let prev_span = self
                            .stmt_span
                            .replace((clause.range().start(), if_stmt.range().end()));
                        self.visit_expr(test);
                        self.stmt_span = prev_span;
                    }
                    self.visit_body(&clause.body);
                }
            }
            _ => ruff_python_ast::visitor::walk_stmt(self, stmt),
        }
        self.stmt_span = prev;
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
                    self.check_call(call, false);
                    self.walk_call_children(call);
                } else {
                    walk_expr(self, expr);
                }
            }
            Expr::Call(call) => {
                self.check_call(call, true);
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

/// Is `code` listed in a `# lint-ignore: a,b` comment on this line?
fn lint_ignored(line: &str, code: &str) -> bool {
    let bytes = line.as_bytes();
    for i in 0..bytes.len() {
        if bytes[i] != b'#' {
            continue;
        }
        let rest = line[i + 1..].trim_start();
        if let Some(after) = rest.strip_prefix("lint-ignore:") {
            let list: String = after
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ',' | '-') || c.is_whitespace())
                .collect();
            if list.split(',').any(|c| c.trim() == code) {
                return true;
            }
        }
    }
    false
}

struct LineIndex {
    starts: Vec<usize>,
    len: usize,
}

impl LineIndex {
    fn new(source: &str) -> Self {
        let mut starts = vec![0usize];
        for (i, b) in source.bytes().enumerate() {
            if b == b'\n' {
                starts.push(i + 1);
            }
        }
        Self {
            starts,
            len: source.len(),
        }
    }

    /// 1-based line number containing byte offset `off`.
    fn line_of(&self, off: usize) -> usize {
        self.starts.partition_point(|&s| s <= off)
    }

    fn line_text<'s>(&self, source: &'s str, line: usize) -> &'s str {
        let start = self.starts[line - 1];
        let end = self.starts.get(line).map_or(self.len, |&e| e - 1);
        &source[start..end]
    }

    fn col_of(&self, off: usize) -> usize {
        off - self.starts[self.line_of(off) - 1] + 1
    }
}

fn check_file(
    path: &Path,
    patterns: &[(String, String)],
    hatches: Hatches,
) -> Vec<(PathBuf, usize, usize, String)> {
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
        hatches,
        in_async: false,
        stmt_span: None,
        diags: Vec::new(),
    };
    for stmt in &parsed.syntax().body {
        checker.visit_stmt(stmt);
    }
    if checker.diags.is_empty() {
        return Vec::new();
    }

    let index = LineIndex::new(&source);
    let n_lines = index.starts.len();
    checker
        .diags
        .into_iter()
        .filter_map(|d| match d {
            Diag::Async200 {
                offset,
                pattern_idx,
            } => {
                let off = usize::from(offset);
                let line = index.line_of(off);
                if noqa_suppresses(index.line_text(&source, line)) {
                    return None;
                }
                let (pat, repl) = &patterns[pattern_idx];
                Some((
                    path.to_path_buf(),
                    line,
                    index.col_of(off),
                    format!(
                        "ASYNC200 User-configured blocking sync call {pat} in async function, consider replacing with {repl}."
                    ),
                ))
            }
            Diag::Hatch {
                code,
                message,
                offset,
                span,
            } => {
                let start_line = index.line_of(usize::from(span.0));
                let end_line = index.line_of(usize::from(span.1)).min(n_lines);
                for line in start_line..=end_line {
                    if lint_ignored(index.line_text(&source, line), code) {
                        return None;
                    }
                }
                Some((
                    path.to_path_buf(),
                    start_line,
                    index.col_of(usize::from(offset)),
                    format!(
                        "{} {message} — rewrite or add `# lint-ignore: {code}` to this line.",
                        code.to_uppercase()
                    ),
                ))
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
    let mut hatches = Hatches::default();

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
        } else if arg == "--type-hatches" || arg.starts_with("--type-hatches=") {
            let v = take_value(&arg, &mut args);
            for code in v.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                match code {
                    "no-any" => hatches.no_any = true,
                    "no-getattr" => hatches.no_getattr = true,
                    "no-object" => hatches.no_object = true,
                    _ => {
                        eprintln!(
                            "error: unknown type-hatch rule {code:?} (valid: {})",
                            HATCH_CODES.join(", ")
                        );
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

    let mut results: Vec<(PathBuf, usize, usize, String)> = files
        .par_iter()
        .flat_map_iter(|f| check_file(f, &patterns, hatches))
        .collect();
    results.sort();

    for (path, line, col, msg) in &results {
        println!("{}:{line}:{col}: {msg}", path.display());
    }
    if results.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
