import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const appDirectory = fileURLToPath(new URL("../", import.meta.url));
const result = spawnSync(
  "wasm-pack",
  [
    "build",
    appDirectory,
    "--target",
    "web",
    "--out-dir",
    "pkg",
    "--out-name",
    "aidash_auth_web",
    ...process.argv.slice(2),
    "--",
    "--locked",
  ],
  {
    cwd: appDirectory,
    // Preserve task-local Cargo directories and an optional WASM_PACK_CACHE.
    env: process.env,
    stdio: "inherit",
  },
);

if (result.error) {
  console.error("Unable to run wasm-pack. Install the README prerequisites.");
  console.error(result.error.message);
}
process.exit(result.status ?? 1);
