import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
export default defineConfig({
  plugins: [react()],
  build: {
    rollupOptions: {
      output: {
        manualChunks: {
          tanstack: [
            "@tanstack/react-router",
            "@tanstack/react-query",
            "@tanstack/react-table",
            "@tanstack/react-form",
            "@tanstack/react-virtual",
          ],
        },
      },
    },
  },
  server: {
    proxy: {
      "/api": process.env.AIDASH_BACKEND ?? "http://127.0.0.1:8080",
      "/auth": process.env.AIDASH_BACKEND ?? "http://127.0.0.1:8080",
      "/.well-known": process.env.AIDASH_BACKEND ?? "http://127.0.0.1:8080",
    },
  },
});
