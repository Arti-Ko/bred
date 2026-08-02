import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

// Порт фиксирован: его же ждёт Tauri в devUrl.
export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    // Пересборка фронтенда не должна триггериться правками Rust.
    watch: { ignored: ['**/src-tauri/**'] },
  },
  build: {
    target: 'esnext',
    // Vite 8 минифицирует через oxc; esbuild-путь объявлен устаревшим
    minify: true,
    sourcemap: false,
  },
});
