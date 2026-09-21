import js from "@eslint/js";
import tseslint from "typescript-eslint";
import hooks from "eslint-plugin-react-hooks";
import jsxA11y from "eslint-plugin-jsx-a11y";
import globals from "globals";
export default tseslint.config(
  {
    ignores: [
      "dist/**",
      "**/generated/**",
      "node_modules/**",
      "test-results/**",
      "playwright-report/**",
    ],
  },
  {
    files: ["**/*.{ts,tsx}"],
    extends: [js.configs.recommended, ...tseslint.configs.recommended],
    languageOptions: { globals: { ...globals.browser, ...globals.node } },
    plugins: { "react-hooks": hooks, "jsx-a11y": jsxA11y },
    rules: {
      ...hooks.configs.recommended.rules,
      // React Compiler is not enabled; TanStack Table/Virtual use mutable instances.
      "react-hooks/incompatible-library": "off",
      ...jsxA11y.configs.recommended.rules,
    },
  },
  {
    files: ["**/*.{js,mjs}"],
    extends: [js.configs.recommended],
    languageOptions: { globals: globals.node },
  },
);
