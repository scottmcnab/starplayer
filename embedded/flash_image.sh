#!/usr/bin/env bash

set -euo pipefail

die() {
    printf 'flash_image.sh: %s\n' "$*" >&2
    exit 1
}

readonly STARPLAYER_EMBEDDED_DIR="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly STARPLAYER_ESP_EXPORT="${HOME}/export-esp-1.97.sh"
readonly STARPLAYER_WEB_IMAGE="${STARPLAYER_EMBEDDED_DIR}/target/starplayer-a1s-web-merged.bin"
readonly STARPLAYER_RFC2217_ENDPOINT="${STARPLAYER_RFC2217_ENDPOINT:-rfc2217://192.168.0.151:8086?ign_set_control}"
readonly STARPLAYER_FLASH_BAUD="${STARPLAYER_FLASH_BAUD:-460800}"

[[ -f "${STARPLAYER_ESP_EXPORT}" && -r "${STARPLAYER_ESP_EXPORT}" ]] ||
    die "Xtensa environment script is missing or unreadable: ${STARPLAYER_ESP_EXPORT}"

# shellcheck source=/dev/null
source "${STARPLAYER_ESP_EXPORT}"

command -v cargo >/dev/null 2>&1 || die "cargo is not available after sourcing ${STARPLAYER_ESP_EXPORT}"
command -v esptool.py >/dev/null 2>&1 || die "esptool.py is not available on PATH"

cd "${STARPLAYER_EMBEDDED_DIR}"
cargo xtask image --board a1s --features web --merge

[[ -f "${STARPLAYER_WEB_IMAGE}" ]] || die "merged web image was not generated: ${STARPLAYER_WEB_IMAGE}"

esptool.py --chip esp32 --baud "${STARPLAYER_FLASH_BAUD}" \
    -p "${STARPLAYER_RFC2217_ENDPOINT}" write_flash 0x0 "${STARPLAYER_WEB_IMAGE}"
