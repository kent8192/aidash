import { spawnSync } from "node:child_process";

const mode = process.argv[2] ?? "release";
const command = mode === "check" ? "cargo" : "wasm-pack";
const args =
  mode === "check"
    ? ["check", "--locked", "--target", "wasm32-unknown-unknown"]
    : [
        "build",
        "--target",
        "web",
        "--out-dir",
        "pkg",
        "--out-name",
        "aidash_marketplace_web",
        mode === "dev" ? "--dev" : "--release",
        "--locked",
      ];
const result = spawnSync(command, args, { stdio: "inherit" });
if (result.error) {
  console.error(
    `${command} could not start. Install Rust 1.96+, the wasm32-unknown-unknown target, and wasm-pack.`,
  );
  process.exit(1);
}
process.exit(result.status ?? 1);
