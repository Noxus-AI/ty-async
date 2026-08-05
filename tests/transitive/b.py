"""Second file: cross-file resolution targets."""
import stripe


def dup():  # same name as in a.py: globally ambiguous
    stripe.dup_b()


def unique_helper():  # globally unique name
    stripe.unique_call()
