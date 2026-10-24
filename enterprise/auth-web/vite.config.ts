import { defineConfig, loadEnv } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, process.cwd(), "AUTHENTICATION_");
  const target = env.AUTHENTICATION_BACKEND_URL || "http://127.0.0.1:8081";
  return {
    plugins: [react()],
    server: {
      proxy: Object.fromEntries(
        ["/login/", "/account", "/logout", "/oauth/", "/.well-known/"].map(
          (path) => [path, { target }],
        ),
      ),
    },
  };
});
