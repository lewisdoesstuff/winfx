<script setup>
// The app screenshot, matching the visitor's colour scheme.
const base = import.meta.env.BASE_URL
const alt =
  'WinFX with the Voice (broadcast) preset: a high-pass filter, gain, expander and a four-band equalizer, with direct mode active on the input device'
</script>

<template>
  <picture>
    <source media="(prefers-color-scheme: dark)" :srcset="base + 'screenshot-dark.png'" />
    <img
      :src="base + 'screenshot-light.png'"
      :alt="alt"
      width="1536"
      height="1095"
      decoding="async"
      class="block h-auto w-full"
    />
  </picture>
</template>
