"""Callers of cross-file functions."""


async def cross_file():
    unique_helper()  # ASYNC200T: globally unique, resolves into b.py


async def ambiguous_call():
    dup()  # NOT reported: two defs of dup(), name-resolution refuses to guess
