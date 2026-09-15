#!/usr/bin/env bash

set -euo pipefail

die() {
    printf 'flash_image_test.sh: %s\n' "$*" >&2
    exit 1
}

readonly SOURCE_EMBEDDED_DIR="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
readonly TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/starplayer flash image test.XXXXXX")"
trap 'rm -rf -- "${TEST_ROOT}"' EXIT

readonly TEST_EMBEDDED_DIR="${TEST_ROOT}/checkout with spaces/embedded"
readonly TEST_HOME="${TEST_ROOT}/home"
readonly FAKE_BIN="${TEST_ROOT}/fake commands"
readonly EVENTS="${TEST_ROOT}/events"
readonly EXPECTED_IMAGE="${TEST_EMBEDDED_DIR}/target/starplayer-a1s-current-merged.bin"
readonly OVERRIDE_IMAGE="${TEST_ROOT}/custom output/voice-bench.bin"
readonly WRONG_IMAGE="${TEST_ROOT}/other checkout/embedded/target/starplayer-a1s-current-merged.bin"

mkdir -p -- "${TEST_EMBEDDED_DIR}" "${TEST_HOME}" "${FAKE_BIN}"
cp -- "${SOURCE_EMBEDDED_DIR}/flash_image.sh" "${TEST_EMBEDDED_DIR}/flash_image.sh"
chmod +x "${TEST_EMBEDDED_DIR}/flash_image.sh"
printf '%s\n' '# Fake test environment; commands are supplied through PATH.' > "${TEST_HOME}/export-esp-1.97.sh"

cat > "${FAKE_BIN}/cargo" <<'FAKE_CARGO'
#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -eq 3 && "$1" == clean && "$2" == -p && "$3" == starplayer-embedded-xtask ]]; then
    [[ ! -e "${FAKE_EXPECTED_IMAGE}" ]] || {
        printf 'stale expected image still existed before cargo clean\n' >&2
        exit 91
    }
    printf '%s\n' cargo-clean >> "${EVENTS}"
elif [[ "$#" -eq 9 && "$1" == xtask && "$2" == image && "$3" == --board && "$4" == a1s && "$5" == --features && "$6" == "${FAKE_EXPECTED_FEATURES}" && "$7" == --merge && "$8" == --out && "$9" == "${FAKE_EXPECTED_IMAGE}" ]]; then
    printf '%s\n' cargo-image >> "${EVENTS}"
    if [[ "${FAKE_BUILD_MODE}" == local ]]; then
        mkdir -p -- "$(dirname -- "${FAKE_EXPECTED_IMAGE}")"
        printf '%s\n' fresh-image > "${FAKE_EXPECTED_IMAGE}"
    elif [[ "${FAKE_BUILD_MODE}" == elsewhere ]]; then
        mkdir -p -- "$(dirname -- "${WRONG_IMAGE}")"
        printf '%s\n' wrong-checkout-image > "${WRONG_IMAGE}"
    else
        printf 'unexpected FAKE_BUILD_MODE: %s\n' "${FAKE_BUILD_MODE}" >&2
        exit 92
    fi
else
    printf 'unexpected cargo invocation:' >&2
    printf ' <%s>' "$@" >&2
    printf '\n' >&2
    exit 93
fi
FAKE_CARGO

cat > "${FAKE_BIN}/esptool.py" <<'FAKE_ESPTOOL'
#!/usr/bin/env bash
set -euo pipefail

printf '%s\n' esptool >> "${EVENTS}"
printf 'arg=%s\n' "$@" >> "${EVENTS}"
FAKE_ESPTOOL
chmod +x "${FAKE_BIN}/cargo" "${FAKE_BIN}/esptool.py"

export PATH="${FAKE_BIN}:${PATH}"
export WRONG_IMAGE EVENTS
export FAKE_EXPECTED_IMAGE="${EXPECTED_IMAGE}"
export FAKE_EXPECTED_FEATURES=web
export STARPLAYER_RFC2217_ENDPOINT='rfc2217://test.invalid:9000?ign_set_control&quoted=yes'
export STARPLAYER_FLASH_BAUD=115200

: > "${EVENTS}"
mkdir -p -- "$(dirname -- "${EXPECTED_IMAGE}")"
printf '%s\n' stale-image > "${EXPECTED_IMAGE}"
FAKE_BUILD_MODE=local HOME="${TEST_HOME}" "${TEST_EMBEDDED_DIR}/flash_image.sh"

cat > "${TEST_ROOT}/expected success events" <<EOF
cargo-clean
cargo-image
esptool
arg=--chip
arg=esp32
arg=--baud
arg=115200
arg=-p
arg=${STARPLAYER_RFC2217_ENDPOINT}
arg=write_flash
arg=0x0
arg=${EXPECTED_IMAGE}
EOF
cmp -s "${TEST_ROOT}/expected success events" "${EVENTS}" || {
    diff -u "${TEST_ROOT}/expected success events" "${EVENTS}" >&2 || true
    die 'successful build did not reach the exact esptool invocation'
}

: > "${EVENTS}"
mkdir -p -- "$(dirname -- "${OVERRIDE_IMAGE}")"
printf '%s\n' stale-override > "${OVERRIDE_IMAGE}"
FAKE_EXPECTED_IMAGE="${OVERRIDE_IMAGE}" FAKE_EXPECTED_FEATURES='voice-bench,web,voice-bench-filtered' \
    STARPLAYER_IMAGE_PATH="${OVERRIDE_IMAGE}" STARPLAYER_FEATURES='voice-bench,web,voice-bench-filtered' \
    FAKE_BUILD_MODE=local HOME="${TEST_HOME}" "${TEST_EMBEDDED_DIR}/flash_image.sh"

cat > "${TEST_ROOT}/expected override events" <<EOF
cargo-clean
cargo-image
esptool
arg=--chip
arg=esp32
arg=--baud
arg=115200
arg=-p
arg=${STARPLAYER_RFC2217_ENDPOINT}
arg=write_flash
arg=0x0
arg=${OVERRIDE_IMAGE}
EOF
cmp -s "${TEST_ROOT}/expected override events" "${EVENTS}" || {
    diff -u "${TEST_ROOT}/expected override events" "${EVENTS}" >&2 || true
    die 'feature and output overrides did not reach the exact cargo and esptool invocations'
}

: > "${EVENTS}"
printf '%s\n' stale-again > "${EXPECTED_IMAGE}"
if FAKE_BUILD_MODE=elsewhere HOME="${TEST_HOME}" "${TEST_EMBEDDED_DIR}/flash_image.sh" > "${TEST_ROOT}/confused stdout" 2> "${TEST_ROOT}/confused stderr"; then
    die 'path-confused build unexpectedly succeeded'
fi

cat > "${TEST_ROOT}/expected confused events" <<'EOF'
cargo-clean
cargo-image
EOF
cmp -s "${TEST_ROOT}/expected confused events" "${EVENTS}" || {
    diff -u "${TEST_ROOT}/expected confused events" "${EVENTS}" >&2 || true
    die 'path-confused build reached an unexpected command'
}
[[ ! -e "${EXPECTED_IMAGE}" ]] || die 'path-confused build left the stale checkout-local image in place'
grep -Fq 'non-empty merged image was not generated in this checkout' "${TEST_ROOT}/confused stderr" ||
    die 'path-confused build did not report the checkout-local image failure'

printf '%s\n' 'flash_image_test.sh: pass'
