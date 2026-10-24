import { build } from "esbuild";
// Inject the official guest plugin only into instrumented native WebViews.
// The shared Web production assets never import the automation module.
await build({
  stdin: {
    contents: 'import "@wdio/tauri-plugin";',
    resolveDir: process.cwd(),
  },
  bundle: true,
  format: "iife",
  platform: "browser",
  target: "es2022",
  outfile: "src-tauri/gen/wdio.js",
});
