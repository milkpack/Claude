import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// The Rust server (apps/photo-studio) serves the JSON API, the OpenArt OAuth routes and the
// health check on one port; in development Vite proxies those to it.
const server = 'http://127.0.0.1:8787';
const proxy = {
  '/api': { target: server, changeOrigin: false },
  '/oauth': { target: server, changeOrigin: false },
  '/healthz': { target: server, changeOrigin: false },
};

export default defineConfig({
  plugins: [react()],
  server: { port: 5173, proxy },
  preview: { port: 4173, proxy },
  build: {
    outDir: 'dist',
    target: 'es2022',
    chunkSizeWarningLimit: 900,
  },
});
