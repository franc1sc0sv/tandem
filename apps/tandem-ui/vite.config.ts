import { defineConfig } from "vite";

// Puerto fijo: `tauri.conf.json` apunta `devUrl` aquí.
export default defineConfig({
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: { target: "es2022" },
});
