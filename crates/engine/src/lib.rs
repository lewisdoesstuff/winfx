//! Audio engine: WASAPI capture/render, DSP chain, clock-drift compensation.
//! Kept UI-agnostic; the app talks to it through lock-free queues.
