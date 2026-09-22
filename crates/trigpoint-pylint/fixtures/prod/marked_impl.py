import time

from trigpoint_shims import DeterministicShim

from shimdefs import ClockShim


class SimClock(ClockShim, DeterministicShim):
    def __init__(self) -> None:
        self.ticks = 0.0

    def now(self) -> float:
        return time.time()
