import path from "path";
import { TanStackRouterVite } from "@tanstack/router-vite-plugin";
import react from "@vitejs/plugin-react-swc";
import { defineConfig } from "vite";

// https://vitejs.dev/config/
export default defineConfig({
  base: "/ui/",
  plugins: [react(), TanStackRouterVite()],
  resolve: {
    alias: {
      "@/lib": path.resolve(__dirname, "./src/lib"),
      "@/components": path.resolve(__dirname, "./src/components"),
    },
  },
  server: {
    proxy: {
      "/v1/": {
        target: "http://localhost:3000",
      },
      "/promptkit.": {
        target: "http://localhost:3000",
      },
    },
  },
  build: {
    chunkSizeWarningLimit: 4096,
    rollupOptions: {
      output: {
        manualChunks: (e) => {
          if (e.includes("/node_modules/monaco-editor/")) return "monaco";
        },
      },
    },
  },
});
