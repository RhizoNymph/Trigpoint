from time import monotonic


def tick() -> float:
    return monotonic()
