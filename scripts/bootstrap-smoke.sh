#!/bin/sh
# End-to-end bootstrap smoke test. It needs real repositories, a runner and
# space for several roots, so it is opt-in and not part of the default CI job.
#
# Usage: RPMX_BOOTSTRAP_SMOKE=1 scripts/bootstrap-smoke.sh MANIFEST
set -eu

if [ "${RPMX_BOOTSTRAP_SMOKE:-0}" != 1 ]; then
    echo "bootstrap smoke is opt-in; set RPMX_BOOTSTRAP_SMOKE=1" >&2
    exit 0
fi

manifest=${1:?usage: RPMX_BOOTSTRAP_SMOKE=1 scripts/bootstrap-smoke.sh MANIFEST}
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
rpmx=${RPMX:-$root/target/release/rpmx}

"$rpmx" bootstrap plan "$manifest" --json >/dev/null
"$rpmx" bootstrap build "$manifest" --force
"$rpmx" bootstrap build "$manifest" --locked --force
