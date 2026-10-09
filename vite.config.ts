import { defineConfig } from 'vite';

export default defineConfig({
  clearScreen: false,
  server: {
    host: '127.0.0.1', port: 1420, strictPort: true,
    // Rust writes and locks executables while building; these are not UI inputs.
    watch: { ignored: ['**/src-tauri/**', '**/artifacts/**', '**/.local/**', '**/.playwright-cli/**'] },
  },
  esbuild: { jsx: 'automatic' },
  build: { target: 'es2022' },
});
