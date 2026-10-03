import assert from "node:assert/strict";
import { spawn, execFileSync } from "node:child_process";
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { startFixture } from "./fixture.mjs";
import { setTimeout as delay } from "node:timers/promises";

const desktop = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const artifacts = resolve(desktop, "artifacts/wdio", process.platform);
const state = await mkdtemp(resolve(tmpdir(), "aidash-wdio-"));
const binary =
  process.env.AIDASH_E2E_BINARY ??
  resolve(
    JSON.parse(
      execFileSync(
        "cargo",
        [
          "metadata",
          "--locked",
          "--no-deps",
          "--format-version=1",
          "--manifest-path",
          "src-tauri/Cargo.toml",
        ],
        { cwd: desktop, encoding: "utf8" },
      ),
    ).target_directory,
    "debug",
    process.platform === "win32" ? "aidash-desktop.exe" : "aidash-desktop",
  );
const fixtures = [];
let env;
let failed = false;
const pids = [];
function run(command, args, environment) {
  return new Promise((resolveRun, reject) => {
    const child = spawn(command, args, {
      cwd: desktop,
      env: environment,
      stdio: "inherit",
      timeout: 240000,
    });
    child.once("error", reject);
    child.once("exit", (code, signal) =>
      code === 0
        ? resolveRun()
        : reject(
            new Error(`Test process failed: code=${code}, signal=${signal}`),
          ),
    );
  });
}
async function stopNative() {
  let pid;
  try {
    pid = Number(await readFile(resolve(state, "pid"), "utf8"));
  } catch {
    return;
  }
  if (!Number.isSafeInteger(pid) || pid <= 0)
    throw new Error("Invalid native test PID");
  const alive = () => {
    try {
      process.kill(pid, 0);
      return true;
    } catch (error) {
      if (error.code === "ESRCH") return false;
      throw error;
    }
  };
  if (!alive()) return;
  process.kill(pid, "SIGTERM");
  for (let attempt = 0; attempt < 50 && alive(); attempt++) await delay(100);
  if (alive()) process.kill(pid, "SIGKILL");
}
try {
  await rm(artifacts, { recursive: true, force: true });
  await mkdir(artifacts, { recursive: true });
  fixtures.push(await startFixture());
  fixtures.push(await startFixture());
  const portProbe = createServer();
  await new Promise((done) => portProbe.listen(0, "127.0.0.1", done));
  const port = portProbe.address().port;
  await new Promise((done) => portProbe.close(done));
  env = {
    ...process.env,
    AIDASH_E2E_BINARY: binary,
    AIDASH_E2E_STATE: state,
    AIDASH_E2E_NODE: process.execPath,
    AIDASH_E2E_BROWSER: resolve(desktop, "tests/wdio/browser.mjs"),
    AIDASH_E2E_ORIGINS: JSON.stringify(
      fixtures.map((fixture) => fixture.origin),
    ),
    TAURI_WEBDRIVER_PORT: String(port),
  };
  // Each launcher exits and terminates the native app before the next phase.
  // Reloading a WebDriver session alone would not prove credential persistence.
  for (const phase of ["login", "restore", "logged-out"]) {
    try {
      await run(
        process.execPath,
        [
          "node_modules/@wdio/cli/bin/wdio.js",
          "run",
          "tests/wdio/wdio.conf.mjs",
        ],
        { ...env, AIDASH_E2E_PHASE: phase },
      );
    } finally {
      await stopNative();
    }
    pids.push(Number(await readFile(resolve(state, "pid"), "utf8")));
  }
  assert.equal(
    new Set(pids).size,
    3,
    "Persistence must cross distinct native processes",
  );
} catch (error) {
  failed = true;
  console.error(error.message);
} finally {
  await stopNative();
  if (env) {
    try {
      await run(binary, [], { ...env, AIDASH_E2E_CLEANUP: "1" });
    } catch {
      failed = true;
      console.error("Failed to remove isolated test credentials");
    }
  }
  await writeFile(
    resolve(artifacts, "results.json"),
    JSON.stringify(
      {
        passed: !failed,
        platform: process.platform,
        arch: process.arch,
        nativePids: pids,
        liveGoogle: false,
        browserLauncher: "external fixture process",
        fixtures: fixtures.map((fixture) => fixture.stats),
      },
      null,
      2,
    ),
  );
  await Promise.all(fixtures.map((fixture) => fixture.close()));
  await rm(state, { recursive: true, force: true });
}
process.exitCode = failed ? 1 : 0;
