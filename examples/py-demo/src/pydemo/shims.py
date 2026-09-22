"""Toy shim library for the trigpylint end-to-end demo.

Everything in this module is inside the simulation import closure, so it must
name zero nondeterminism sinks. The production clock lives in `pydemo.prod`
precisely because it may not be imported from sim scope.
"""

from typing import Protocol

from trigpoint_shims import DeterministicShim


class ClockShim(Protocol):
    """The time seam: every read of the clock goes through this protocol."""

    def now_millis(self) -> int:
        """Return the current time in milliseconds."""
        ...


class SimClock(ClockShim, DeterministicShim):
    """Simulation clock: virtual time, fully deterministic.

    Marked with `DeterministicShim`, so the `[[python.shims]]` grant of the
    `time` capability does not apply here: naming a sink inside this class
    would be a broken determinism claim, not a blessed introduction.
    """

    def __init__(self, start: int = 0) -> None:
        self._now = start

    def now_millis(self) -> int:
        self._now += 1
        return self._now


def run_tick(clock: ClockShim) -> int:
    """Consume the shim generically: any `ClockShim` will do."""
    return clock.now_millis()
