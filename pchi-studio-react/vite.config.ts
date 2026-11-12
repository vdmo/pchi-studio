import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import { fileURLToPath } from 'node:url'

// Studio talks to PCHI Core only through its network API (never shares UI
// code with the Rune console). The conductor's dashboard WS/HTTP is on
// :3000; its governance-event WS is on :8889.
export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      '@pchi/sdk': fileURLToPath(new URL('../pchi-sdk/src/index.ts', import.meta.url)),
    },
  },
  server: {
    port: 5173,
    fs: { allow: ['..'] },
    proxy: {
      '/core': {
        target: 'http://127.0.0.1:3000',
        ws: true,
        rewrite: (p) => p.replace(/^\/core/, ''),
      },
    },
  },
})
