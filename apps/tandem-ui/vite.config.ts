import { svelte } from "@sveltejs/vite-plugin-svelte";
import { defineConfig } from "vitest/config";

// Puerto fijo: `tauri.conf.json` apunta `devUrl` aquí. Dos páginas (relativas a esta
// carpeta): la ventana principal y la de la barra de menú / bandeja.
//
// `--mode demo` (`pnpm build:demo`) deja el modo demo dentro del build: lo usa el arnés de
// capturas (`pnpm shots`), y también la galería de componentes (`gallery.html`), con la que el
// arnés compara estilos con los mockups. El build normal (el de Tauri) no incluye ninguno.
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
