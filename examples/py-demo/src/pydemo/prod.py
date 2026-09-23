"""Production shim implementations: the blessed nondeterminism introduction points.

`SystemClock` subclasses the declared `ClockShim` protocol, so the
`[[python.shims]]` grant of the `time` capability covers the `time.time`
reference in its method — that is what prod mode allows. Sim mode allows no
such thing, which is why this module is outside the harness import closure.
"""

import time

from pydemo.shims import ClockShim


class SystemClock(ClockShim):
    """Production clock: reads real wall time.

    Fine in production, a violation the moment it reaches a simulation build.
    """

    def now_millis(self) -> int:
        return int(time.time() * 1000)
