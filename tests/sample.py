import stripe

async def f(ctx):
    stripe.Charge.create()          # match stripe.*
    await stripe.Charge.create()    # awaited -> no match
    ctx.storage.save()              # match ctx.storage.*
    x = obj.load_data()             # match *.load_data
    y = markdownify("hi")           # match markdownify
    stripe.foo()                    # noqa
    stripe.bar()                    # noqa: ASYNC200
    stripe.baz()                    # noqa: E501
    def inner():
        stripe.inner_call()         # sync context -> no match
    async def inner2():
        stripe.inner_call2()        # match
    cb = lambda: stripe.lam()       # lambda -> no match

def sync_fn():
    stripe.sync_call()              # no match
