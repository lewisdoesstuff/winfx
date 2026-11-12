# Security policy

## Reporting a vulnerability

Please report security issues privately, not in a public issue. Use **Report a vulnerability** on the repository's [Security tab](../../security/advisories/new). You'll get a reply within a week.

Include what you found, the steps to reproduce it, and the WinFX and Windows versions you tested.

## Scope

These parts of WinFX run with more privileges than the app, so they matter most:

- **The direct-mode plug-in** (`winfx_apo.dll`), which runs inside the Windows audio engine as LOCAL SERVICE and reads its settings from `%ProgramData%\WinFX\shared.bin`.
- **The elevated setup and removal commands** (`winfx.exe --apo-setup` and `--apo-remove`), which register the plug-in and change audio device settings.
- **The installer and uninstaller.**

Only the latest release gets fixes.
