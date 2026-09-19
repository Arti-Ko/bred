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
    // Два окна — две точки входа: мессенджер и бой, который открывается
    // отдельным окном и мессенджера вокруг себя не имеет.
    rollupOptions: {
      input: { index: 'index.html', gundyr: 'gundyr.html' },
    },
    target: 'esnext',
    // Vite 8 минифицирует через oxc; esbuild-путь объявлен устаревшим
    minify: true,
    sourcemap: false,
  },
});
