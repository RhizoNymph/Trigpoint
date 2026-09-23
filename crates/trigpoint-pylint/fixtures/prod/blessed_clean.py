import time

from shimdefs import ClockShim


class SystemClock(ClockShim):
    def now(self) -> float:
        return time.time()

    def sleep(self, seconds: float) -> None:
        time.sleep(seconds)
