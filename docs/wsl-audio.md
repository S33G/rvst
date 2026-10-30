# Audio on WSL

[Documentation](README.md) / Audio on WSL

WSL2 has no sound hardware, so the native runners (`rvst-standalone`,
`rvst-platform`) have no ALSA devices to open. Route WSL audio to a server that
owns the hardware — WSLg's bundled PulseAudio, or PulseAudio/PipeWire on the
Windows host — and CPAL keeps working through ALSA unchanged.

## Detect the connection

Run from the framework checkout or a generated plugin repository:

```sh
. scripts/audio-env.sh --verbose
eval "$(scripts/audio-env.sh)"
```

The script prints `export PULSE_SERVER=...` and keeps any value you already set.
Precedence:

1. Caller-exported `PULSE_SERVER`
2. Linux PulseAudio/PipeWire server on `/mnt/wslg/PulseServer` (standard WSLg)
3. TCP to the Windows host, port 4713 (host overridable with `RVST_WSL_AUDIO_HOST`)

## WSL setup

```sh
sudo apt install -y pulseaudio-utils libasound2-plugins
. scripts/audio-env.sh
pactl info
speaker-test -c 2 -t sine
```

`pactl info` must show the detected server and `speaker-test` must be heard in
Windows output before blaming any RVST runner.

### ALSA default device

CPAL opens ALSA's `default` PCM, which normally expands to hardware card 0. WSL
has no sound cards, so point the default at Pulse once:

```sh
cat > ~/.asoundrc <<'EOF'
# Route ALSA's default device to the PulseAudio server (WSLg socket).
pcm.!default {
    type pulse
}
ctl.!default {
    type pulse
}
EOF
```

Without it CPAL fails with ``Unknown PCM default`` and "audio device is not
available" even after `libasound2-plugins` is installed.

### Expected underruns

The WSLg sink starts `SUSPENDED`; its first buffers often underrun, and the
runner reports `audio stream or processor reported errors` for a single stream
error. Rerun — a clean full-duration run is the acceptance signal. Persistent
underruns mean the Windows output device is busy; close other audio apps or
raise the Pulse sink buffer instead of changing RVST latency budgets.

## Windows host server (TCP route)

If WSLg's default socket is silent or you serve several WSL distributions, run
PulseAudio or [PipeWire](https://gitlab.freedesktop.org/pipewire/pipewire) on
Windows and accept `module-native-protocol-tcp` connections on port 4713 from
the WSL subnet. Then:

```sh
RVST_WSL_AUDIO_HOST=<windows-host-ip> . scripts/audio-env.sh
```

## Use the runner

Normal commands need no extra flags or code changes:

```sh
cargo run -p plugin-standalone
cargo rvst inspect --open
```

## Limitations

| Symptom | Cause |
| --- | --- |
| No `PulseServer` socket | WSLg missing (WSL below 0.64) or disabled |
| `pactl` connects but audio is silent | Windows mixer routes the sink elsewhere; check output device in Windows settings |
| Latency grows under load | Plain Pulse/PipeWire TCP has 20–50 ms round trip; tune sink buffer, not RVST |
| `--no-ui` run fails to open device | Same routing problem; probe with `pactl info` first |
