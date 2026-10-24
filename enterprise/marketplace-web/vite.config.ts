import { defineConfig, loadEnv } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, process.cwd(), "MARKETPLACE_");
  const target = env.MARKETPLACE_BACKEND_URL || "http://127.0.0.1:8082";
  return {
    plugins: [react()],
    server: {
      proxy: {
        "/api/marketplace": { target },
        "/auth": { target },
      },
    },
  };
});
