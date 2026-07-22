#!/bin/sh
# Shell builtins keep this fixture focused on output capping. Spawning dd/tr
# pipelines made the fixture itself occasionally exceed the probe deadline on
# busy test runners, turning an output-boundary test into a process-timeout one.
printf '%02048d' 0
printf '%02048d' 0 >&2
printf ' TS_SECRET_OVERSIZE 1.2.3\n'
