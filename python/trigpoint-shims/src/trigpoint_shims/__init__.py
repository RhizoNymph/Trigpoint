"""Marker base classes consumed by trigpoint's Python determinism linter.

This package must stay dependency-free and stable: analyzed projects depend
on it solely to *declare* intent, never for behavior.
"""

__all__ = ["DeterministicShim"]


class DeterministicShim:
    """Declares that a shim implementation is deterministic.

    Subclass this on the simulation variant of a shim (e.g. ``SimClock`` for a
    ``ClockShim`` protocol). The contract: no nondeterminism sink — time,
    random, I/O, threads, subprocesses, environment — is reachable through any
    method of the marked type. The linter verifies the contract over the
    import closure of the declared simulation roots; a sink named inside a
    marked type is reported as a broken determinism claim.

    The class is a pure marker: it defines no methods, no attributes and no
    ``__init__``, so subclassing it changes nothing at runtime.
    """

    __slots__ = ()
