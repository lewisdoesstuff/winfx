// The site's copy. Every claim comes from the repository README; don't add anything it doesn't say.

const repo = 'https://github.com/lewisdoesstuff/winfx'

export const links = {
  download: repo + '/releases/latest',
  source: repo,
  troubleshooting: repo + '#troubleshooting',
  outputModes: repo + '#output-modes',
  license: repo + '/blob/main/LICENSE',
  easyEffects: 'https://github.com/wwmm/easyeffects',
}

export const icon = import.meta.env.BASE_URL + 'icon.png'

export const tagline = 'Real-time effects for your microphone on Windows.'
export const summary =
  'Build a chain of effects, hear what each one does, and every app that uses your mic gets the processed sound.'
export const facts = ['Free and open source', 'Works offline']

export const effectGroups = [
  {
    name: 'Dynamics',
    items: ['Gate', 'Expander', 'Compressor', 'De-esser', 'Auto-gain', 'Maximizer', 'Limiter'],
  },
  {
    name: 'Tone',
    items: ['High-pass filter', 'Low-pass filter', 'Parametric EQ', 'Exciter', 'Bass enhancer', 'Convolver'],
    note: 'The EQ has up to 32 bands over a live spectrum. The convolver loads impulse responses.',
  },
  {
    name: 'Fun',
    items: ['Pitch shift', 'Harmonizer', 'Autotune', 'Reverb', 'Delay', 'Bit crusher'],
  },
]

export const tools = [
  {
    key: 'clip',
    name: 'Test clip',
    text: 'Record yourself, loop the clip through the chain, and switch between processed and original.',
  },
  {
    key: 'calibrate',
    name: 'One-click calibration',
    text: 'Fits the gain and gate thresholds to your mic and room.',
  },
  {
    key: 'presets',
    name: 'Presets',
    text: 'Start from one of three presets, save your own, or import from EasyEffects, Equalizer APO and AutoEQ.',
  },
  {
    key: 'offline',
    name: 'Offline',
    text: 'WinFX never connects to the internet: no account, telemetry or update checks.',
  },
  {
    key: 'native',
    name: 'Tray and themes',
    text: 'Follows your Windows light or dark theme, sits in the tray, and can start at login.',
  },
]

export const modes = [
  {
    key: 'direct',
    name: 'Direct',
    badge: 'Recommended',
    text: 'Apps keep using your mic as normal and get the processed sound, with nothing to reconfigure.',
    points: [
      'Adds a millisecond or two of latency.',
      'One-time setup per device, with one admin prompt.',
      'Keeps running with your last chain after you quit WinFX.',
    ],
    path: ['Microphone', 'Windows audio engine, with WinFX inside', 'Every app'],
  },
  {
    key: 'cable',
    name: 'Virtual cable',
    badge: 'Fallback',
    text: 'Your apps record from the cable instead of the mic.',
    points: [
      "For mics whose driver doesn't support Windows audio enhancements.",
      "For when you can't get admin rights for the setup.",
      'Keeps the raw mic available alongside the processed one.',
      'Adds a few milliseconds of latency.',
    ],
    path: ['Microphone', 'WinFX', 'Virtual cable', 'Your apps'],
  },
]

export const steps = [
  {
    title: 'Download and run the installer',
    text: "It installs for your account only and doesn't need admin rights.",
  },
  {
    title: 'Choose your microphone',
    text: 'Open WinFX and pick it under Input.',
  },
  {
    title: 'Set up direct mode',
    text: 'Under Output, select Set up. Windows asks for permission once, and audio restarts briefly.',
  },
  {
    title: 'Calibrate',
    text: 'Select Calibrate in the Input card. Stay quiet for 4 seconds, then speak normally for 5.',
  },
  {
    title: 'Pick a preset',
    text: 'Adjust it to taste, using Test clip to hear the changes.',
  },
]

export const smartScreen = {
  title: "The installer isn't code-signed yet",
  text: 'Windows SmartScreen may warn you about it. Select More info, then Run anyway.',
}

export const requirements = [
  { label: 'System', value: 'Windows 10 version 1809 or later, x64' },
  { label: 'Tested on', value: 'Windows 11' },
  { label: 'Direct mode', value: 'Windows audio enhancements on for the mic, and admin rights once for setup' },
  { label: 'Virtual cable mode', value: 'A virtual audio cable such as VB-Audio Virtual Cable' },
]

export const footer = {
  credit: 'Inspired by',
  creditName: 'EasyEffects',
  creditTail: 'on Linux.',
  disclaimer: 'WinFX is an independent open-source project, not affiliated with Microsoft.',
}
