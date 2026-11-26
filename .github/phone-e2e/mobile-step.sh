#!/usr/bin/env bash
# mobile.py with the tauri-driver it expects already running, both with this run's isolated endpoint, data folder and seams.
set +e
root="$(cygpath -w "$PWD")"
mkdir -p e2e-work/mobile e2e-work/mobile-data e2e-out/mobile-shots
export KELPIE_ENDPOINT=kelpie-e2e-mobile-ci
export KELPIE_DATA_DIR="$root\\e2e-work\\mobile-data"
export KELPIE_LOAD_FORCE=L0:calm KELPIE_TEST_USER_PRESENCE=approve KELPIE_E2E=1
export KELPIE_TAILSCALE_BIN="$root\\e2e-work\\mobile\\fake-tailscale.cmd"
export KELPIE_PUSH_TEST_BASE=http://127.0.0.1:47391
export KELPIE_DAEMON_PATH="$root\\target\\debug\\kelpied.exe"
export KELPIE_E2E_APP="$root\\target\\debug\\kelpie.exe"
case "$TAURI_DRIVER" in
  *.cmd) python -X utf8 scripts/e2e/attach_driver.py --port 4444 --native-driver "$MSEDGEDRIVER" > e2e-out/mobile-tauri-driver.log 2>&1 & ;;
  *) tauri-driver --port 4444 --native-driver "$MSEDGEDRIVER" > e2e-out/mobile-tauri-driver.log 2>&1 & ;;
esac
sleep 3
python -X utf8 scripts/e2e/mobile.py "$root\\e2e-out\\mobile-shots" "$root\\e2e-work\\mobile" --edgedriver "$EDGE_DRIVER"
code=$?
for image in tauri-driver.exe msedgedriver.exe kelpie.exe kelpied.exe; do
  MSYS_NO_PATHCONV=1 taskkill /F /T /IM "$image" > /dev/null 2>&1
done
exit $code
