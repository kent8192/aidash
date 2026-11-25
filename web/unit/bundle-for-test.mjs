import { build } from "esbuild";
import { createRequire } from "node:module";

export async function bundleForTest(entry, input) {
  const result = await build({
    ...input,
    bundle: true,
    platform: "node",
    format: "cjs",
    write: false,
  });
  const module = { exports: {} };
  new Function("require", "module", "exports", result.outputFiles[0].text)(
    createRequire(entry),
    module,
    module.exports,
  );
  return module.exports;
}
