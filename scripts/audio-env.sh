#!/bin/sh
# Print PulseAudio/PipeWire connection settings that route WSL audio to Windows.
#
# The native runners (rvst-standalone, rvst-platform) use CPAL/ALSA. WSL2 has no
# sound cards, so audio clients must connect to a server that owns hardware:
# WSLg's bundled PulseAudio socket, or PulseAudio/PipeWire on the Windows host
# listening on TCP.
#
# The script is evaluation-safe: source it or capture the exports below.
#   . scripts/audio-env.sh --verbose
# or
#   eval "$(scripts/audio-env.sh)"
#
# A caller-exported PULSE_SERVER is preserved unchanged. Without a WSLg socket
# and a reachable Windows host the script fails with a settable override name.

set -u

verbose=0
note() {
    [ "$verbose" -eq 1 ] && printf '# %s\n' "$1" >&2
    return 0
}

report() {
    note "source: $1"
    printf 'export %s=%s\n' "$2" "$3"
}

for arg in "$@"; do
    case "$arg" in
        --verbose|-v) verbose=1 ;;
        *) printf '%s: unknown option %s\n' "$0" "$arg" >&2; exit 2 ;;
    esac
done

# Callers often set PULSE_SERVER themselves (mirrored networking, custom ports).
if [ -n "${PULSE_SERVER:-}" ]; then
    report 'pre-existing PULSE_SERVER' 'PULSE_SERVER' "$PULSE_SERVER"
    exit 0
fi

# WSLg ships a PulseAudio server inside the utility VM. Its Unix socket accepts
# any local client. Depending on the WSL release audio leaves through the RDP
# sink or stays in the browser sandbox; test with pactl or speaker-test.
if [ -S /mnt/wslg/PulseServer ]; then
    report 'WSLg PulseAudio socket' 'PULSE_SERVER' 'unix:/mnt/wslg/PulseServer'
    exit 0
fi

# WSL2 without a WSLg socket: reach a Pulse/PipeWire server on the Windows host
# over TCP. The host is the DNS resolver from /etc/resolv.conf unless mirrored
# networking makes localhost valid; override with RVST_WSL_AUDIO_HOST.
host="${RVST_WSL_AUDIO_HOST:-}"
if [ -z "$host" ] && [ -r /etc/resolv.conf ]; then
    host=$(awk '/^nameserver/ { print $2; exit }' /etc/resolv.conf 2>/dev/null)
fi
if [ -z "$host" ] && command -v ip >/dev/null 2>&1; then
    host=$(ip route show default 2>/dev/null | awk '{ for (i = 1; i <= NF; i++) if ($i == "via") { print $(i + 1); exit } }')
fi
if [ -z "$host" ]; then
    printf '%s: no WSLg socket and no Windows host detected; set RVST_WSL_AUDIO_HOST\n' "$0" >&2
    exit 1
fi

report "TCP to Windows host $host" 'PULSE_SERVER' "tcp:$host:4713"
