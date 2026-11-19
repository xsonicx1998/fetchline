import { defineConfig } from 'vitest/config'
import react from '@vitejs/plugin-react'

// Frontend unit tests (Round 75): jsdom + React Testing Library.
// Run with `npm run test:ui`. Dev-only dependencies; this config
// is not loaded by `vite build`, so the shipped app is unaffected.
export default defineConfig({
  plugins: [react()],
  test: {
    environment: 'jsdom',
    globals: true,
    include: ['src/**/*.test.{ts,tsx}'],
  },
})
