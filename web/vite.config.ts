import { defineConfig, type UserConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { fileURLToPath } from "node:url";
import { dependencies } from "./package.json";
const here = (path: string) => fileURLToPath(new URL(path, import.meta.url));

/**
 * The one Vite configuration of the dashboard. `@slots` resolves to the stubs
 * in `src/slots`. A build that extends the dashboard passes the directory of
 * its own sources, and its `src/slots` replaces the stubs.
 */
export function dashboardConfig(extension?: string): UserConfig {
  const backend = process.env.AIDASH_BACKEND ?? "http://127.0.0.1:8080";
  return {
    plugins: [react(), tailwindcss()],
    resolve: {
      alias: {
        "@": here("./src"),
        "@slots": extension ? `${extension}/src/slots` : here("./src/slots"),
      },
      // Sources outside this package have no node_modules above them, so
      // they import the packages installed here.
      dedupe: extension ? Object.keys(dependencies) : [],
    },
    build: {
      ...(extension && { outDir: `${extension}/dist`, emptyOutDir: true }),
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
      ...(extension && { fs: { allow: [here("."), extension] } }),
      proxy: { "/api": backend, "/auth": backend, "/.well-known": backend },
    },
  };
}

export default defineConfig(dashboardConfig());
