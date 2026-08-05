# ty-async

Extremely fast linter implementing flake8-async's **ASYNC200** check
(user-configured blocking calls in `async` functions), built on
[ruff](https://github.com/astral-sh/ruff)'s Python parser. Drop-in
replacement for the flake8 flag, ~2 orders of magnitude faster.

```bash
pip install ty-async

ty-async src-dir another-dir \
  --exclude=src-dir/tests \
  --async200-blocking-calls='stripe.*->alt(),*.load_data->(),fcntl.flock->()'
```

## Stdlib preset

There is no official CPython list of "don't call this in the event loop",
but flake8-async's ASYNC21x–25x rules are the de-facto one. `--async200-stdlib`
appends a curated preset mirroring them plus high-signal extras: `time.sleep`,
`input`, `open`/`io.open`/`os.fdopen`, pathlib `read_text`/`write_bytes`/…,
heavy `shutil`/`os.walk`/`tempfile` filesystem work, `subprocess.*` and
`os.system`/`os.spawn*`/`os.wait*`, `urllib.request.urlopen`, `socket`
DNS lookups, `smtplib`/`ftplib`/`http.client`, `sqlite3.connect`, and
archive/compression helpers. Deliberately omitted: generic method names
(`.read`, `.recv`, `.acquire`, `.wait`, `.get`) and cheap single-syscall
metadata ops (`os.stat`, `os.listdir`, `os.mkdir`, all of `os.path`) —
too many false positives / too little blocking to matter. User patterns win on overlap, awaited calls stay exempt (so
`aiofiles`/`anyio` wrappers never fire), and it composes with
`--async200-transitive`.

## Transitive mode

`--async200-transitive` (with the same patterns flag) additionally builds a
name-based call graph and reports call sites in async functions that reach a
blocking call *through* sync helpers, with the chain as evidence:

```
app.py:161:18: ASYNC200T blocking sync call markdown.markdown reachable from async function via markdown_to_slack_blocks (client.py:176:12)
```

Calls are resolved by name only (same class → same file → globally unique;
ambiguous names are skipped, not guessed). Callables merely passed to
`asyncio.to_thread` / `run_in_executor` are not calls, so executor wrappers
are naturally exempt. Dynamic dispatch and framework callbacks are out of
scope. Suppress a site with `# noqa` / `# noqa: ASYNC200` on its line.

## Type escape hatches

`--type-hatches=no-any,no-getattr,no-object` enables three extra rules
(usable alone or together with the ASYNC200 flag, single pass either way):

- `no-getattr` — bare `getattr()` / `hasattr()` calls
- `no-any` — `Any` (or `typing.Any`, `t.Any`, …) inside an annotation
- `no-object` — `object` inside an annotation

Suppress a site with a trailing `# lint-ignore: no-any,no-getattr` comment
anywhere in the enclosing statement / function-signature span.

## Semantics

Semantics match flake8-async exactly: calls inside `async def` bodies
(nested `def`/`lambda` reset the context), directly-awaited calls are
skipped, patterns are fnmatch-ed against `ast.unparse(func)` (so `*`
crosses dots), `# noqa` / `# noqa: ASYNC200` are honored, and output
format and exit codes match flake8. Only ASYNC200 is implemented;
`--ignore` and `--asyncio` are accepted as no-ops.
