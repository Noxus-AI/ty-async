"""Transitive ASYNC200 fixture: same-file chains."""
import asyncio
import stripe


def leaf():
    stripe.Charge.create()  # direct blocking hit in a sync function


def helper():
    leaf()


async def root():
    helper()  # ASYNC200T: root -> helper -> leaf


async def direct():
    stripe.Charge.create()  # plain ASYNC200 only, no transitive report


class C:
    def m(self):
        stripe.method_call()

    async def am(self):
        self.m()  # ASYNC200T via same-class method resolution


def to_thread_target():
    stripe.wrapped()


async def safe():
    await asyncio.to_thread(to_thread_target)  # passed, not called: no report


def r1():
    r2()


def r2():
    r1()
    stripe.cycle_call()


async def cyclic():
    r1()  # ASYNC200T: cycle must not hang, still reaches the blocking call


async def middle_async():
    helper()  # reported here; async fns are roots, never traversed through


async def outer_async():
    await middle_async()  # NOT reported: callee is async, has its own report


def dup():
    stripe.dup_a()


async def noqa_site():
    helper()  # noqa: ASYNC200


def clean_helper():
    return 1


async def clean_root():
    clean_helper()  # no blocking anywhere: no report
