<script setup>
// Calm and native: soft layered surfaces in the app's accent blue.
import Screenshot from './Screenshot.vue'
import {
  effectGroups,
  facts,
  footer,
  icon,
  links,
  modes,
  requirements,
  smartScreen,
  steps,
  summary,
  tagline,
  tools,
} from './content.js'

const toolIcons = {
  clip: 'M4 12a8 8 0 0 1 13.7-5.6M20 12a8 8 0 0 1-13.7 5.6M18 3v4h-4M6 21v-4h4',
  calibrate: 'M4 7h8m6 0h2M4 17h2m6 0h8M15 4.5a2.5 2.5 0 1 0 0 5a2.5 2.5 0 0 0 0-5zM9 14.5a2.5 2.5 0 1 0 0 5a2.5 2.5 0 0 0 0-5z',
  presets: 'M12 4l8 4.5-8 4.5-8-4.5L12 4zM4 13l8 4.5 8-4.5',
  offline: 'M12 3l7 3v5c0 4.4-2.9 8-7 10-4.1-2-7-5.6-7-10V6l7-3zM9 12l2 2 4-4',
  native: 'M4 5h16v14H4zM4 9h16',
}

const nav = [
  { href: '#effects', label: 'Effects' },
  { href: '#modes', label: 'How it works' },
  { href: '#start', label: 'Getting started' },
]
</script>

<template>
  <div class="relative min-h-screen overflow-x-clip bg-page font-sans text-ink antialiased">
    <!-- Soft colour wash behind the hero, like a wallpaper tint showing through -->
    <div aria-hidden="true" class="wash pointer-events-none absolute inset-x-0 top-0 h-[56rem]"></div>

    <header class="relative mx-auto flex max-w-6xl items-center gap-6 px-4 py-4 sm:px-6">
      <a href="#" class="flex items-center gap-2.5 rounded-md text-[15px] font-semibold no-underline">
        <img :src="icon" alt="" width="28" height="28" class="size-7" />
        WinFX
      </a>
      <nav aria-label="Sections" class="ml-auto hidden items-center gap-1 text-sm sm:flex">
        <a
          v-for="item in nav"
          :key="item.href"
          :href="item.href"
          class="rounded-md px-3 py-1.5 text-muted hover:bg-surface hover:text-ink"
        >
          {{ item.label }}
        </a>
      </nav>
      <a
        :href="links.source"
        class="ml-auto rounded-md border border-line bg-surface px-3 py-1.5 text-sm hover:bg-raised sm:ml-0"
      >
        GitHub
      </a>
    </header>

    <main class="relative">
      <!-- Hero -->
      <section class="mx-auto max-w-6xl px-4 pt-10 text-center sm:px-6 sm:pt-16">
        <h1 class="mx-auto max-w-3xl font-display text-4xl leading-[1.1] font-semibold tracking-tight text-balance sm:text-6xl">
          {{ tagline }}
        </h1>
        <p class="mx-auto mt-5 max-w-2xl text-lg text-pretty text-muted">{{ summary }}</p>

        <div class="mt-8 flex flex-col items-stretch justify-center gap-3 sm:flex-row sm:items-center">
          <a
            :href="links.download"
            class="on-fill inline-flex items-center justify-center gap-2 rounded-md bg-accent px-5 py-2.5 font-semibold text-on-accent shadow-sm hover:opacity-90"
          >
            <svg aria-hidden="true" viewBox="0 0 24 24" class="size-5" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <path d="M12 4v11m0 0l-4.5-4.5M12 15l4.5-4.5M5 20h14" />
            </svg>
            Download for Windows
          </a>
          <a
            :href="links.source"
            class="inline-flex items-center justify-center gap-2 rounded-md border border-line bg-raised px-5 py-2.5 font-semibold shadow-sm hover:bg-surface"
          >
            View source
          </a>
        </div>

        <ul class="mt-6 flex flex-wrap items-center justify-center gap-x-6 gap-y-1 text-sm text-muted">
          <li v-for="fact in facts" :key="fact" class="flex items-center gap-2">
            <span aria-hidden="true" class="size-1.5 rounded-full bg-accent"></span>{{ fact }}
          </li>
        </ul>

        <div class="mx-auto mt-12 max-w-5xl overflow-hidden rounded-lg border border-line bg-raised shadow-[0_32px_64px_-24px_rgb(0_0_0/0.35),0_2px_8px_rgb(0_0_0/0.08)] sm:rounded-xl">
          <Screenshot />
        </div>
      </section>

      <!-- Effects -->
      <section id="effects" class="mx-auto max-w-6xl px-4 pt-20 sm:px-6 sm:pt-28">
        <h2 class="font-display text-3xl font-semibold tracking-tight">What it does</h2>

        <div class="mt-8 grid gap-3 md:grid-cols-3">
          <article
            v-for="group in effectGroups"
            :key="group.name"
            class="rounded-lg border border-line bg-surface p-5"
          >
            <h3 class="font-semibold">{{ group.name }}</h3>
            <ul class="mt-4 flex flex-wrap gap-2">
              <li
                v-for="item in group.items"
                :key="item"
                class="rounded-md border border-line bg-raised px-2.5 py-1 text-sm"
              >
                {{ item }}
              </li>
            </ul>
            <p v-if="group.note" class="mt-4 text-sm text-muted">{{ group.note }}</p>
          </article>
        </div>

        <div class="mt-3 grid gap-3 sm:grid-cols-2 lg:grid-cols-6">
          <article
            v-for="(tool, index) in tools"
            :key="tool.key"
            class="flex gap-4 rounded-lg border border-line bg-surface p-5"
            :class="index < 2 ? 'lg:col-span-3' : 'lg:col-span-2'"
          >
            <span class="grid size-9 shrink-0 place-items-center rounded-md bg-accent-soft text-accent">
              <svg aria-hidden="true" viewBox="0 0 24 24" class="size-5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
                <path :d="toolIcons[tool.key]" />
              </svg>
            </span>
            <div>
              <h3 class="font-semibold">{{ tool.name }}</h3>
              <p class="mt-1 text-sm text-muted">{{ tool.text }}</p>
            </div>
          </article>
        </div>
      </section>

      <!-- Output modes -->
      <section id="modes" class="mx-auto max-w-6xl px-4 pt-20 sm:px-6 sm:pt-28">
        <h2 class="font-display text-3xl font-semibold tracking-tight">How it works</h2>

        <div class="mt-8 grid gap-3 lg:grid-cols-2">
          <article
            v-for="mode in modes"
            :key="mode.key"
            class="rounded-lg border bg-surface p-5 sm:p-6"
            :class="mode.key === 'direct' ? 'border-accent' : 'border-line'"
          >
            <div class="flex items-center gap-3">
              <h3 class="text-xl font-semibold">{{ mode.name }}</h3>
              <span
                class="rounded-full px-2.5 py-0.5 text-xs font-semibold"
                :class="mode.key === 'direct' ? 'bg-accent text-on-accent' : 'border border-line text-muted'"
              >
                {{ mode.badge }}
              </span>
            </div>

            <ol class="mt-5 flex flex-wrap items-center gap-x-1.5 gap-y-2 text-sm" :aria-label="mode.name + ' signal path'">
              <li v-for="(stop, index) in mode.path" :key="stop" class="flex items-center gap-1.5">
                <svg v-if="index > 0" aria-hidden="true" viewBox="0 0 16 16" class="size-4 text-muted" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                  <path d="M6 3.5L10.5 8 6 12.5" />
                </svg>
                <span class="rounded-md border border-line bg-raised px-2.5 py-1">{{ stop }}</span>
              </li>
            </ol>

            <p class="mt-5 text-pretty">{{ mode.text }}</p>
            <ul class="mt-4 space-y-2 text-sm text-muted">
              <li v-for="point in mode.points" :key="point" class="flex gap-2.5">
                <svg aria-hidden="true" viewBox="0 0 16 16" class="mt-0.5 size-4 shrink-0 text-accent" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                  <path d="M3 8.5l3.2 3.2L13 5" />
                </svg>
                {{ point }}
              </li>
            </ul>
          </article>
        </div>
        <p class="mt-4 text-sm text-muted">
          <a :href="links.outputModes" class="text-accent underline underline-offset-4">Output modes in the README</a>
        </p>
      </section>

      <!-- Getting started -->
      <section id="start" class="mx-auto max-w-6xl px-4 pt-20 sm:px-6 sm:pt-28">
        <h2 class="font-display text-3xl font-semibold tracking-tight">Getting started</h2>

        <div class="mt-8 grid items-start gap-3 lg:grid-cols-[1.4fr_1fr]">
          <div>
            <ol class="overflow-hidden rounded-lg border border-line bg-surface">
              <li
                v-for="(step, index) in steps"
                :key="step.title"
                class="flex gap-4 border-line p-5"
                :class="index > 0 ? 'border-t' : ''"
              >
                <span aria-hidden="true" class="grid size-7 shrink-0 place-items-center rounded-full bg-accent-soft text-sm font-semibold text-accent">
                  {{ index + 1 }}
                </span>
                <div>
                  <h3 class="font-semibold">{{ step.title }}</h3>
                  <p class="mt-1 text-sm text-muted">{{ step.text }}</p>
                </div>
              </li>
            </ol>
          </div>

          <div class="grid gap-3">
            <aside class="flex gap-3 rounded-lg border border-note-line bg-note p-5">
              <svg aria-hidden="true" viewBox="0 0 20 20" class="mt-0.5 size-5 shrink-0" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
                <path d="M10 3l7.5 13h-15L10 3zM10 8.5v3.5M10 14.2v.1" />
              </svg>
              <div>
                <h3 class="font-semibold">{{ smartScreen.title }}</h3>
                <p class="mt-1 text-sm">{{ smartScreen.text }}</p>
              </div>
            </aside>

            <section id="requirements" class="rounded-lg border border-line bg-surface p-5">
              <h3 class="font-semibold">Requirements</h3>
              <dl class="mt-3 text-sm">
                <div
                  v-for="(row, index) in requirements"
                  :key="row.label"
                  class="grid grid-cols-[7.5rem_1fr] gap-3 border-line py-2.5"
                  :class="index > 0 ? 'border-t' : ''"
                >
                  <dt class="text-muted">{{ row.label }}</dt>
                  <dd>{{ row.value }}</dd>
                </div>
              </dl>
            </section>

            <a
              :href="links.download"
              class="on-fill inline-flex items-center justify-center rounded-md bg-accent px-5 py-2.5 font-semibold text-on-accent shadow-sm hover:opacity-90"
            >
              Download the latest release
            </a>
            <a :href="links.troubleshooting" class="justify-self-center text-sm text-accent underline underline-offset-4">
              Troubleshooting
            </a>
          </div>
        </div>
      </section>
    </main>

    <footer class="relative mx-auto mt-24 max-w-6xl border-t border-line px-4 pt-8 pb-20 text-sm text-muted sm:px-6">
      <div class="flex flex-wrap items-center gap-x-6 gap-y-3">
        <span class="flex items-center gap-2 font-semibold text-ink">
          <img :src="icon" alt="" width="20" height="20" class="size-5" />WinFX
        </span>
        <a :href="links.source" class="underline underline-offset-4 hover:text-ink">Source</a>
        <a :href="links.license" class="underline underline-offset-4 hover:text-ink">MIT license</a>
        <a :href="links.troubleshooting" class="underline underline-offset-4 hover:text-ink">Troubleshooting</a>
      </div>
      <p class="mt-4">
        {{ footer.credit }}
        <a :href="links.easyEffects" class="underline underline-offset-4 hover:text-ink">{{ footer.creditName }}</a>
        {{ footer.creditTail }} {{ footer.disclaimer }}
      </p>
    </footer>
  </div>
</template>

<style scoped>
.wash {
  background:
    radial-gradient(40rem 26rem at 18% 8%, var(--glow-1), transparent 70%),
    radial-gradient(36rem 24rem at 82% 4%, var(--glow-2), transparent 70%),
    radial-gradient(44rem 26rem at 50% 42%, var(--glow-3), transparent 70%);
}
</style>
