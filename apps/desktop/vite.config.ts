import { defineConfig } from "vite";

export default defineConfig({
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  envPrefix: ["VITE_"],
  build: { target: "es2022" },
});
