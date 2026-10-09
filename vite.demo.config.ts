import { defineConfig } from 'vite';

// Explicit development entry. The production configuration never imports it.
export default defineConfig({
  clearScreen: false,
  server: {
    host: '127.0.0.1', port: 1421, strictPort: true,
    watch: { ignored: ['**/src-tauri/**', '**/artifacts/**', '**/.local/**'] },
  },
  esbuild: { jsx: 'automatic' },
  build: { target: 'es2022', outDir: 'artifacts/ui-demo-dist', rollupOptions: { input: 'ui-demo.html' } },
});
