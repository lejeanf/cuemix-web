import { defineConfig } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import pkg from './package.json' with { type: 'json' };

// The build version the page compares with the server's (stale tab reload); the dev server is "0.0.0-dev".
export default defineConfig(({ command }) => ({
  base: './',
  define: { __APP_VERSION__: JSON.stringify(command === 'build' ? pkg.version : '0.0.0-dev') },
  plugins: [svelte()],
  test: { environment: 'node', include: ['src/**/*.test.ts'] },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
  },
  server: {
    proxy: {
      '/ws': { target: 'ws://127.0.0.1:8080', ws: true },
      '/api': { target: 'http://127.0.0.1:8080' },
    },
  },
}));
