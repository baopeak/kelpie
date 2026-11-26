#!/usr/bin/env bash
# Runs one e2e script: its output goes to e2e-out/<name>.log (and the step's log), its exit code and duration to
# e2e-out/<name>.exit. A step killed by its timeout leaves no .exit file, which the summary reports as a timeout.
#
#   .github/phone-e2e/run-one.sh <name> <command...>
set +e
name="$1"
shift
mkdir -p e2e-out
echo "== $name: $*"
start=$(date +%s)
"$@" 2>&1 | tee "e2e-out/$name.log"
code=${PIPESTATUS[0]}
echo "$code $(( $(date +%s) - start ))" > "e2e-out/$name.exit"
echo "== $name: exit $code"
exit "$code"
