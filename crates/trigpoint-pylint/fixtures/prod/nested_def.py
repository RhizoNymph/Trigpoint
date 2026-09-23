import time

from shimdefs import ClockShim


class SystemClock(ClockShim):
    def now(self) -> float:
        def read() -> float:
            return time.time()

        return read()


def unblessed_outer() -> float:
    def read() -> float:
        return time.time()

    return read()
