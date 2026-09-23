import time


def line_level() -> float:
    return time.time()  # triglint: allow(shim-nondeterminism)


# triglint: allow(shim-nondeterminism)
def def_level() -> float:
    return time.time()


def standalone_above() -> float:
    # triglint: allow(shim-nondeterminism)
    return time.time()


def not_allowed() -> float:
    return time.time()
