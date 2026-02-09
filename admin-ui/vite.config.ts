import { fileURLToPath, URL } from 'node:url'

import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import vueDevTools from 'vite-plugin-vue-devtools'
import ui from '@nuxt/ui/vite'

// https://vite.dev/config/
export default defineConfig({
  base: '/admin/',
  plugins: [
    vue(),
    vueDevTools(),
    ui(),
  ],
  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url))
    },
  },
  server: {
    proxy: {
      '/admin/auth': {
        target: 'http://localhost:4096',
        changeOrigin: true,
      },
      '/admin/oauth': {
        target: 'http://localhost:4096',
        changeOrigin: true,
      },
      '/admin/keys': {
        target: 'http://localhost:4096',
        changeOrigin: true,
      },
      '/admin/swagger': {
        target: 'http://localhost:4096',
        changeOrigin: true,
      },
      '/admin/api-docs': {
        target: 'http://localhost:4096',
        changeOrigin: true,
      },
    },
  },
})
