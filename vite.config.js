import { defineConfig } from 'vite';
import { svelte, vitePreprocess } from '@sveltejs/vite-plugin-svelte';

// The Svelte front end lives in ui/ and builds into dist/, which Tauri bundles.
export default defineConfig({
  root: 'ui',
  plugins: [svelte({ preprocess: vitePreprocess() })],
  clearScreen: false,
  server: { port: 1420, strictPort: true, watch: { ignored: ['**/app/**', '**/engine/**', '**/target/**'] } },
  build: { outDir: '../dist', emptyOutDir: true, target: 'safari15', chunkSizeWarningLimit: 2000 },
});
