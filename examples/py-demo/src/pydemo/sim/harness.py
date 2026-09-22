"""Simulation harness: the sim root declared in triglint.toml.

The import closure of this module is deterministic and lints clean. Importing
`pydemo.violate` from here pulls real time and randomness into sim scope,
which trigpylint must reject.
"""

from pydemo.shims import SimClock, run_tick


def main() -> None:
    clock = SimClock(start=0)
    tick = run_tick(clock)
    print(f"sim tick at virtual t={tick}")


if __name__ == "__main__":
    main()
