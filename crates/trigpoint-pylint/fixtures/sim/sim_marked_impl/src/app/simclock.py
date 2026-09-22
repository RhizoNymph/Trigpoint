import time

from trigpoint_shims import DeterministicShim


class ClockShim:
    def now(self) -> float:
        raise NotImplementedError


class SimClock(ClockShim, DeterministicShim):
    def now(self) -> float:
        return time.time()
