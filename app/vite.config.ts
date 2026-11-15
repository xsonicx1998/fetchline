import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

// Tauri expects the built UI in `dist/`. The dev server runs on 1420.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    target: 'es2021',
  },
})
