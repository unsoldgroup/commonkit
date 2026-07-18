import { defineConfig } from "vite";

export default defineConfig({
  clearScreen: false,
  server: { strictPort: true },
  envPrefix: ["VITE_"],
  build: { target: "es2022" },
});
