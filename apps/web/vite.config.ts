import { fileURLToPath, URL } from 'node:url';
import tailwindcss from '@tailwindcss/vite';
import react from '@vitejs/plugin-react';
import { defineConfig } from 'vitest/config';

export default defineConfig({
  plugins: [react(), tailwindcss()],
  test: {
    // Pure model tests only: the session rail, transcript projection and
    // reducer are framework-free by design, so no DOM environment is needed.
    include: ['src/**/*.test.ts'],
    environment: 'node',
  },
  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url)),
      '@agent-fleet/protocol-types': fileURLToPath(
        new URL('../../packages/protocol-types/src/index.ts', import.meta.url),
      ),
      '@agent-fleet/api-client': fileURLToPath(
        new URL('../../packages/api-client/src/index.ts', import.meta.url),
      ),
    },
  },
  server: {
    port: 5173,
    proxy: {
      '/api': {
        target: 'http://127.0.0.1:8787',
        changeOrigin: true,
        ws: true,
      },
      '/ws': {
        target: 'http://127.0.0.1:8787',
        changeOrigin: true,
        ws: true,
      },
    },
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    // Monaco is bundled locally (no runtime CDN); a single ~4 MB chunk is
    // expected and cached by the browser.
    chunkSizeWarningLimit: 5000,
  },
});
