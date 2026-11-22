import { defineConfig } from 'vitest/config'
import react from '@vitejs/plugin-react'
import { fileURLToPath } from 'node:url'

// UI smoke tests: components rendered in jsdom against a stand-in runtime (a mocked fetch and WebSocket). `npm run test:ui`.
// They do not draw pixels or run a real browser, so they cannot see layout or colour: look at a change in the app too.
export default defineConfig({
  plugins: [react()],
  resolve: { alias: { '@pchi/sdk': fileURLToPath(new URL('../pchi-sdk/src/index.ts', import.meta.url)) } },
  test: {
    environment: 'jsdom',
    include: ['test/ui/**/*.test.{ts,tsx}'],
    setupFiles: ['test/ui/setup.ts'],
    testTimeout: 10000,
  },
})
