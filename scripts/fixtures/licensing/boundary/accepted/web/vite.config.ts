import { defineConfig } from "vite";
const here = (path: string) => new URL(path, import.meta.url).pathname;
export default defineConfig({
  resolve: {
    alias: {
      "@": here("./src"),
      "@slots": here("./src/slots"),
    },
  },
});
