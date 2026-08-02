"""Fixture for --type-hatches rules. Expected output: tests/hatches.expected."""

from typing import Any


# --- no-any: every spelling of typing.Any in annotation position ---
def f_bare(x: Any) -> None: ...
def f_t_alias(x: t.Any) -> None: ...
def f_typing(x: typing.Any) -> None: ...
def f_tp_alias(x: tp.Any) -> None: ...
def f_typing_ext(x: typing_extensions.Any) -> None: ...
def f_other_base(x: foo.Any) -> None: ...  # not the typing module: no hit
def f_string_ann(x: "Any") -> None: ...  # string annotation: no hit
def f_nested(x: dict[str, Any]) -> list[Any]: ...  # two hits
def f_return_only() -> Any: ...


# every parameter kind
def f_params(a: Any, /, b: object, *args: Any, c: Any = 1, **kw: object): ...


# --- no-object ---
def f_object(x: object) -> None: ...


y_object: object = 1
y_no_ann = object()  # constructor call, not an annotation: no hit


# --- suppression ---
def f_ignored(x: Any) -> None: ...  # lint-ignore: no-any
def f_multiline(
    x: Any,
) -> object: ...  # lint-ignore: no-any,no-object
y_both: Any = 2  # lint-ignore: no-any


class C:
    attr: Any

    def method(self) -> Any: ...


@some.decorator
def f_decorated(q: Any): ...  # reported on the def line, not the decorator


# --- no-getattr ---
def g(obj):
    getattr(obj, "a")
    hasattr(obj, "b")
    obj.getattr("c")  # attribute access, not the builtin: no hit
    x = getattr(obj, "d")  # lint-ignore: no-getattr
    y = (
        hasattr(obj, "e"))  # lint-ignore: no-getattr
    if hasattr(obj, "f"):  # reported on the if line
        pass
    elif hasattr(obj, "g"):  # reported on the elif line
        pass


ld = lambda z=getattr(y_no_ann, "x"): z


async def aw(obj):
    return await getattr(obj, "coro")
