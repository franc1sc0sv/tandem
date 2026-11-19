import { svelte } from "@sveltejs/vite-plugin-svelte";
import { defineConfig } from "vitest/config";

// Fixed port: `tauri.conf.json` points `devUrl` here. Two pages (relative to this
// folder): the main window and the menu bar / tray one.
//
// `--mode demo` (`pnpm build:demo`) keeps demo mode inside the build: the screenshot
// harness (`pnpm shots`) uses it, and so does the component gallery (`gallery.html`), which
// the harness compares against the mockups for styles. The normal build (Tauri's) includes neither.
export default defineConfig(({ mode }) => ({
  plugins: [svelte()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  define: {
    __DEMO__: JSON.stringify(mode !== "production"),
  },
  build: {
    target: "es2022",
    rollupOptions: {
      input: { main: "index.html", tray: "tray.html", ...(mode === "production" ? {} : { gallery: "gallery.html" }) },
    },
  },
  resolve: process.env.VITEST ? { conditions: ["browser"] } : undefined,
  test: {
    environment: "jsdom",
    include: ["src/**/*.test.ts", "scripts/**/*.test.ts"],
    setupFiles: ["src/test/setup.ts"],
  },
}));
