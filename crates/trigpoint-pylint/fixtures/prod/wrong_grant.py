import random

from shimdefs import ClockShim


class SystemClock(ClockShim):
    def jitter(self) -> float:
        return random.random()
