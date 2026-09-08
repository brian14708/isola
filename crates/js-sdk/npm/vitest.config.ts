import path from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";

const __dirname = path.dirname(fileURLToPath(import.meta.url));

export default defineConfig({
  root: __dirname,
  resolve: {
    alias: [
      {
        find: /^(.*\/)?isola\.cjs$/,
        replacement: path.resolve(__dirname, "dist/isola.cjs"),
      },
    ],
  },
  test: {
    testTimeout: 120000,
    hookTimeout: 120000,
  },
});
