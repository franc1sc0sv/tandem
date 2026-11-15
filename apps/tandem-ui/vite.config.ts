import { defineConfig } from "vite";

// Puerto fijo: `tauri.conf.json` apunta `devUrl` aquí. Dos páginas (relativas a esta
// carpeta): la ventana principal y la mini escena del ícono de la barra de menú / bandeja.
export default defineConfig({
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: {
    target: "es2022",
    rollupOptions: { input: { main: "index.html", tray: "tray.html" } },
  },
});
