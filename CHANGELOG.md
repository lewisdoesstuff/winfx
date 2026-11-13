# Changelog

## Unreleased

- Installing or upgrading while WinFX is running now works in silent installs (winget). Setup asks WinFX to quit, instead of showing a prompt that a silent install answers with Cancel, and starts it again in the tray afterwards.

## 0.1.0 (2026-11-12)

First release.

- Effect chain with dynamics (gate, expander, compressor, de-esser, auto-gain, maximizer, limiter), tone (filters, parametric EQ with live spectrum, exciter, bass enhancer, convolver) and fun effects (pitch shift, harmonizer, autotune, reverb, delay, bit crusher).
- Direct mode (the default): effects run inside the Windows audio engine on the microphone itself, with a one-time setup from the app.
- Virtual cable mode for use with VB-Audio Virtual Cable or similar.
- Test clip recording with processed and original playback, and one-click calibration.
- Starter presets, saved presets, and import from EasyEffects, Equalizer APO and AutoEQ.
- Per-user installer, tray icon, and start at login.
