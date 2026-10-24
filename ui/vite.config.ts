import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri expects a fixed dev port and serves the built files from ui/dist.
export default defineConfig(({ command }) => ({
  plugins: [react()],
  // public/ only holds icons for the browser demo; the app reads them from the bundled game folders.
  publicDir: command === "serve" ? "public" : false,
  clearScreen: false,
  server: { port: 1420, strictPort: true, host: "127.0.0.1" },
  build: {
    outDir: "dist",
    target: "es2022",
    sourcemap: false,
    // Two pages: the app and the in-game notification window.
    rollupOptions: { input: { main: "index.html", overlay: "overlay.html" } },
  },
}));
