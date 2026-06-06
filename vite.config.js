import { defineConfig } from "vite";
import path from "path";

const frontendDir = path.resolve(__dirname, "frontend");

export default defineConfig({
  root: frontendDir,
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    outDir: path.resolve(frontendDir, "dist"),
    emptyOutDir: true,
  },
});
