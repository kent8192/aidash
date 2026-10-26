import { defineConfig } from "vite";
export default defineConfig({
  resolve: {
    alias: {
      "@gcpi": "enterprise/web/src/gcip",
    },
  },
});
