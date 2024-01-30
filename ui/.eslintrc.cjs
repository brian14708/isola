module.exports = {
  root: true,
  env: { browser: true, es2020: true },
  extends: [
    "eslint:recommended",
    "plugin:@typescript-eslint/recommended",
    "plugin:react-hooks/recommended",
    "plugin:tailwindcss/recommended",
  ],
  ignorePatterns: ["dist", ".eslintrc.cjs", "src/routeTree.gen.ts"],
  parser: "@typescript-eslint/parser",
  overrides: [
    {
      files: ["src/components/ui/*.tsx"],
      rules: {
        "tailwindcss/enforces-shorthand": "off",
      },
    },
  ],
  rules: {
    "tailwindcss/migration-from-tailwind-2": "off",
  },
};
