import { defineConfig, loadEnv } from "vite";

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, process.cwd(), "MARKETPLACE_");
  const target = env.MARKETPLACE_BACKEND_URL || "http://127.0.0.1:8082";
  return {
    build: { target: "es2022" },
    server: {
      proxy: {
        "/api/marketplace": { target },
        "/auth": { target },
      },
    },
  };
});
