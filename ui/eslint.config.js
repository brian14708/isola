import eslint from "@eslint/js";
import hooksPlugin from "eslint-plugin-react-hooks";
import tailwindPlugin from "eslint-plugin-tailwindcss";
import tseslint from "typescript-eslint";

export default tseslint.config(
  eslint.configs.recommended,
  ...tseslint.configs.recommended,
  {
    plugins: {
      "react-hooks": hooksPlugin,
    },
    rules: hooksPlugin.configs.recommended.rules,
  },
  {
    plugins: {
      tailwindcss: tailwindPlugin,
    },
    rules: tailwindPlugin.configs.recommended.rules,
  },
  {
    ignores: ["dist", "src/routeTree.gen.ts", "packages/api/*"],
  },
  {
    files: ["src/components/ui/*.{ts,tsx}"],
    rules: {
      "tailwindcss/enforces-shorthand": "off",
      "tailwindcss/no-custom-classname": "off",
    },
  },
  {
    rules: {
      "tailwindcss/migration-from-tailwind-2": "off",
    },
  },
);
