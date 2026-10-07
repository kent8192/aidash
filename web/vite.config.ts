import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { fileURLToPath } from "node:url";
export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: { alias: { "@": fileURLToPath(new URL("./src", import.meta.url)) } },
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
