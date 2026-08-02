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

async def g(ctx, cond):
    stripe.checkout.Session.create()    # deep chain: * crosses dots
    get_client().load_data()            # call-result base matches *.load_data
    ctx.storage.get("a").load_data()    # two matches at the same position
    await gather(stripe.inner())        # inner call of awaited call: match
    xs = [stripe.comp() for _ in ys]    # comprehensions keep async context
    if cond:
        pass
    elif stripe.in_elif():              # elif test: exactly one match
        pass
    del stripe.also[ctx.storage.del_(1)]  # match inside del target

async def precedence():
    stripe.bar2.thing()                 # first matching pattern wins
