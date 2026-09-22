import time

from shimdefs import ClockShim


class SystemClock(ClockShim):
    def now(self) -> float:
        read = lambda: time.time()
        return read()
