# Contributing to WinFX

Thanks for helping out. Bug reports, presets, effect ideas and pull requests are all welcome.

For anything bigger than a small fix, open an issue first so we can agree on the approach before you put time into it.

## Setup

You need Windows 10 or 11, Rust (stable, MSVC toolchain) and the Windows SDK.

```
cargo build --release
cargo test --workspace
```

Before opening a pull request, run:

```
cargo fmt --all
cargo clippy --workspace
cargo test --workspace
```

The repo uses LF line endings everywhere (`.gitattributes` enforces this). Set `core.autocrlf` to `false` or `input` so Git doesn't convert files on checkout.

## Working on the app

`winfx.exe --preview` runs the app with a synthetic engine: no audio devices are opened, nothing is saved, and meters show simulated speech. It's the quickest way to work on the UI.

Interface text follows a few rules:

- Short, plain labels in sentence case, in the style of Windows 11 settings.
- No captions that restate a label, and no hints for problems the user hasn't hit.
- ASCII only: no em dashes, arrows or curly quotes.
- Generic wording: no particular apps, brands or hardware.

## Working on direct mode

`crates/apo` builds `winfx_apo.dll`, which Windows loads into its audio engine (`audiodg.exe`, running as LOCAL SERVICE). Mistakes here affect every app on the PC, so take extra care:

- **A crash or an error silences the mic for every app.** If the plug-in fails to load, Windows delivers digital silence on that device.
- **`process` runs on a real-time thread.** It must not allocate, lock, log or do I/O. New settings arrive through the watcher thread, which hands chains over without blocking.
- **The app and the plug-in share `%ProgramData%\WinFX\shared.bin`.** Treat its contents as untrusted input in the plug-in.
- **Windows caches each device's plug-in list.** Changes to the DLL or its registration only take effect after the audio service restarts, which needs admin.
- **Windows silences all capture while the session is locked**, so test with the PC unlocked.

`tools\apo-dev` sets up scheduled tasks that install the DLL and restart audio without a permission prompt each time. Run `setup.ps1` once, elevated, and `teardown.ps1` to remove everything it added. See the comments at the top of each script.

These examples help with testing:

- `apo_probe` records a device and reports levels.
- `apo_ctl` pushes a chain, toggles bypass and shows status.
- `apo_info` checks whether a device is ready for direct mode.

Run each one with `cargo run -p engine --example <name>`.

## Commits

Keep commits focused, with a one-line summary in the imperative ("Add a tilt EQ"), followed by more detail if it helps.
