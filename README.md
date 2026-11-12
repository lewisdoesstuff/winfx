# WinFX

Real-time effects for your microphone on Windows, in the spirit of
[EasyEffects](https://github.com/wwmm/easyeffects). Build a chain of effects,
see what each one is doing, and every app that records your mic gets the
processed sound.

- **Dynamics:** gate, expander, compressor, de-esser, auto-gain, maximizer, limiter
- **Tone:** high/low-pass, up to 32-band parametric EQ with a live spectrum, exciter, bass enhancer, convolver (impulse responses)
- **Fun:** pitch shift, harmonizer, autotune, reverb, delay, bit crusher
- **Tuning tools:** record a test clip and loop it through the chain with A/B against the original; one-click calibration of gain and gate thresholds to your mic
- **Presets:** save your own; import EasyEffects presets (`.json`) and Equalizer APO / AutoEQ files (`.txt`)
- Fluent design with Mica, light and dark themes following Windows, tray icon, start at login

## How the processed sound reaches other apps

WinFX has two output modes. Pick one in the **Output** card.

### Direct (recommended)

The effects run inside the Windows audio engine, on the input device itself.
Apps keep using your normal microphone and get the processed sound, with no
added latency beyond the limiter's look-ahead (about 1 ms).

- Needs a **one-time setup** per device: select **Set up** in the Output card.
  Windows asks for permission once, and audio restarts briefly.
- **Audio enhancements** must be on for the device (Settings > System > Sound >
  your input). WinFX offers to turn them on if they're off.
- Apps that open the mic in exclusive mode bypass Windows' effects, so they hear the raw mic.
- The effects keep running after you close WinFX, with the last chain you set.
- To undo the setup, select **Remove** in the General card.

Some audio interface drivers create a separate set of Windows devices for each
sample rate. WinFX attaches to all of them during setup, and to new ones as
they appear.

### Virtual cable

WinFX records the mic, processes it, and plays the result into a virtual audio
cable such as [VB-Audio Virtual Cable](https://vb-audio.com/Cable/). Apps record
from the cable's output ("CABLE Output") instead of the mic. Needs no setup
beyond installing the cable, but adds the cable's latency. With VB-Cable, set its
latency to 3 × 256 samples or more in its control panel; smaller values can
produce silence.

## Install

1. Download the installer from the releases page and run it. It installs for
   your account only and doesn't need admin rights.
2. Start WinFX, choose your microphone in **Input**, and pick an **Output** mode.
3. Select **Calibrate** in the Input card, stay quiet for 4 seconds, then speak
   for 5. WinFX sets the gain and gate thresholds for your mic and room.

Settings and presets live in `%APPDATA%\winfx`. Direct mode keeps its shared
state and log in `%ProgramData%\WinFX`.

## Building from source

Requirements: Windows 10 or 11, Rust (stable, MSVC toolchain), and the Windows SDK.

```
cargo build --release
```

This produces `target\release\winfx.exe` and `target\release\winfx_apo.dll`
(the direct-mode effects plug-in, which must sit next to the exe).

```
cargo test --workspace
```

### Layout

| Crate | What it is |
|---|---|
| `crates/engine` | Audio I/O (WASAPI), the DSP effects, presets and import, the direct-mode shared memory and setup |
| `crates/app` | The WinFX app (GPUI) |
| `crates/apo` | `winfx_apo.dll`, the audio processing object that runs the chain inside the Windows audio engine |

Useful tools: `cargo run -p engine --example run -- --list` (headless engine),
`--example analyze` (compare presets on a recording), `--example apo_ctl`
(drive the direct-mode plug-in), and `tools\apo-dev` (scripts for developing the
plug-in without repeated admin prompts).

## License

MIT. See [LICENSE](LICENSE) and `THIRD-PARTY-NOTICES.html` for the libraries WinFX uses.
