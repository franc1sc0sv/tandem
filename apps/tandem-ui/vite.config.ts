import { defineConfig } from "vite";

// Puerto fijo: `tauri.conf.json` apunta `devUrl` aquí. Tres páginas (relativas a esta
// carpeta): la ventana principal, la mini escena del ícono de la barra de menú / bandeja y
// la sonda de calibración a pantalla completa (T-078).
export default defineConfig({
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: {
    target: "es2022",
    rollupOptions: { input: { main: "index.html", tray: "tray.html", probe: "probe.html" } },
  },
});
