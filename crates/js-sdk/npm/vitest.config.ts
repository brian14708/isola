import path from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";

const __dirname = path.dirname(fileURLToPath(import.meta.url));

export default defineConfig({
  root: __dirname,
  resolve: {
    alias: [
      {
        find: /^(.*\/)?isola\.js$/,
        replacement: path.resolve(__dirname, "dist/isola.js"),
      },
    ],
  },
  test: {
    testTimeout: 120000,
    hookTimeout: 120000,
  },
});
