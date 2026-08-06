import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";

export default defineConfig({
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  envPrefix: ["VITE_"],
  build: {
    target: "es2022",
    rollupOptions: {
      input: {
        main: fileURLToPath(new URL("index.html", import.meta.url)),
        tray: fileURLToPath(new URL("tray.html", import.meta.url)),
      },
    },
  },
});
