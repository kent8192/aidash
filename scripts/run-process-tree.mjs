import { spawn } from "node:child_process";

const [command, ...args] = process.argv.slice(2);

if (!command) {
  console.error("Usage: node scripts/run-process-tree.mjs <command> [args...]");
  process.exit(2);
}

const child = spawn(command, args, {
  detached: true,
  stdio: "inherit",
});

let stopping = false;
let killTimer;
let spawnFailed = false;
const osSignalNumbers = {
  SIGHUP: 1,
  SIGINT: 2,
  SIGKILL: 9,
  SIGTERM: 15,
};

function signalProcessTree(signal) {
  if (stopping) return;
  stopping = true;

  if (child.pid !== undefined) {
    try {
      process.kill(-child.pid, signal);
    } catch (error) {
      if (error.code !== "ESRCH") throw error;
    }
  }

  if (child.pid !== undefined) {
    killTimer = setTimeout(() => {
      try {
        process.kill(-child.pid, "SIGKILL");
      } catch (error) {
        if (error.code !== "ESRCH") throw error;
      }
    }, 5000);
  }
}

process.on("SIGINT", () => signalProcessTree("SIGINT"));
process.on("SIGTERM", () => signalProcessTree("SIGTERM"));
process.on("SIGHUP", () => signalProcessTree("SIGHUP"));

child.on("error", (error) => {
  spawnFailed = true;
  console.error(`Failed to start ${command}: ${error.message}`);
  process.exitCode = 127;
});

child.on("exit", () => {
  if (!stopping) signalProcessTree("SIGTERM");
});

child.on("close", (code, signal) => {
  clearTimeout(killTimer);
  if (spawnFailed) {
    process.exitCode = 127;
  } else if (code !== null) {
    process.exitCode = code;
  } else if (signal) {
    process.exitCode = 128 + (osSignalNumbers[signal] ?? 1);
  }
});
