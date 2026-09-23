"""Shim protocol declarations shared by the prod fixtures."""

from trigpoint_shims import DeterministicShim


class ClockShim:
    def now(self) -> float:
        raise NotImplementedError


class EntropyShim:
    def random_float(self) -> float:
        raise NotImplementedError
