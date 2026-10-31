# Audio on WSL

[Documentation](README.md) / Audio on WSL

WSL2 has no sound hardware, so the native runners (`rvst-standalone`,
`rvst-platform`) have no ALSA devices to open. Route WSL audio to a server that
owns the hardware — WSLg's bundled PulseAudio, or PulseAudio/PipeWire on the
Windows host — and CPAL keeps working through ALSA unchanged.

## Detect the connection

Run from the framework checkout. If the plugin repository vendors its SDK, use
`vendor/rvst/scripts/audio-env.sh` as the helper path instead:

```sh
audio_script=scripts/audio-env.sh
# In a generated project with a vendored SDK, use:
# audio_script=vendor/rvst/scripts/audio-env.sh
rvst_audio_env="$("$audio_script" --verbose)" && eval "$rvst_audio_env"
```

The script prints a shell-quoted `export PULSE_SERVER=...` only after detecting
WSL. Capture and evaluate it as shown; an existing value is kept unchanged.
Precedence:

1. Caller-exported `PULSE_SERVER`
2. Linux PulseAudio/PipeWire server on `/mnt/wslg/PulseServer` (standard WSLg)
3. TCP to the Windows host, port 4713 (host overridable with `RVST_WSL_AUDIO_HOST`)

## WSL setup

```sh
sudo apt install -y pulseaudio-utils libasound2-plugins
audio_script=scripts/audio-env.sh
# In a generated project with a vendored SDK, use:
# audio_script=vendor/rvst/scripts/audio-env.sh
rvst_audio_env="$("$audio_script" --verbose)" &&
    eval "$rvst_audio_env" &&
    pactl info &&
    speaker-test -c 2 -t sine
```

`pactl info` must show the detected server and `speaker-test` must be heard in
Windows output before blaming any RVST runner.

### ALSA default device

CPAL opens ALSA's `default` PCM, which normally expands to hardware card 0. WSL
has no sound cards, so point the default at Pulse once:

Put these stanzas in `~/.asoundrc`. If that file already exists, back it up and
merge the stanzas so you keep any device-specific settings:

```text
pcm.!default {
    type pulse
}
ctl.!default {
    type pulse
}
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
windows_host_ip=192.168.1.10 # replace with the Windows host address
audio_script=scripts/audio-env.sh
# In a generated project with a vendored SDK, use:
# audio_script=vendor/rvst/scripts/audio-env.sh
rvst_audio_env="$(RVST_WSL_AUDIO_HOST="$windows_host_ip" "$audio_script")" && eval "$rvst_audio_env"
```

Keep the Windows firewall rule limited to the WSL virtual network. Do not
expose the PulseAudio TCP port to other networks.

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
