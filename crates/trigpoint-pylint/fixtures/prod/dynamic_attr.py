import time


def read(name: str):
    return getattr(time, name)


def literal_is_fine():
    return getattr(time, "timezone")
