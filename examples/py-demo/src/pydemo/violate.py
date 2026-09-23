"""Deliberate determinism violations, quarantined out of the sim import closure.

Nothing imports this module. It is the Python analog of sim-demo's `violate`
cargo feature: the payload trigpylint must reject, parked where the default
run cannot see it. Import it from `pydemo.sim.harness` and sim mode fails with
witness chains `pydemo.sim.harness -> pydemo.violate -> ...`.

Prod mode scans every collected module, including this one, so the two sinks
below carry the documented allow comments — the `#[allow(shim_nondeterminism)]`
escape hatch, exactly as sim-demo's `wait_a_bit` does. Sim mode ignores them:
an allow silences the introduction rule, not the simulation contract.
"""

import random  # triglint: allow(shim-nondeterminism)

from pydemo.prod import SystemClock
from pydemo.shims import run_tick


def coin_flip() -> bool:
    """Nondeterminism hidden behind a plain function: a module-fence sink."""
    # triglint: allow(shim-nondeterminism)
    return random.random() < 0.5


def wall_clock_tick() -> int:
    """Route the real clock into the generic sim path."""
    return run_tick(SystemClock())
