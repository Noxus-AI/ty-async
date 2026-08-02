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
