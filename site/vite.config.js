import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import tailwindcss from '@tailwindcss/vite'

// GitHub Pages project site, served from /winfx/.
export default defineConfig({
  base: '/winfx/',
  plugins: [vue(), tailwindcss()],
})
