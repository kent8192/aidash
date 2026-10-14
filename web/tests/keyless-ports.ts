import { renameSync, writeFileSync } from "node:fs";
import type { Server } from "node:http";
import type { AddressInfo } from "node:net";
import { join } from "node:path";

/**
 * Loopback ports the e2e nodes permit for Keyless endpoints (#256). A Keyless
 * model or embedding endpoint reaches a private address only when the
 * Deployment lists its exact origin, so `scripts/golden_path.py` reserves one
 * port per spec below, permits it, and holds it for the whole run, forwarding
 * it to the port the spec publishes. Elsewhere the fixture's own port is used.
 */
export const KeylessFixture = {
  semantic: 0,
  generation: 1,
  scopedAccess: 2,
} as const;

/**
 * Serves a Keyless fixture on any free loopback port and returns the origin
 * the nodes must be given for it.
 */
export async function serveKeyless(
  server: Server,
  fixture: (typeof KeylessFixture)[keyof typeof KeylessFixture],
): Promise<string> {
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const port = (server.address() as AddressInfo).port;
  const reserved = Number(
    (process.env.AIDASH_E2E_KEYLESS_PORTS ?? "").split(",")[fixture],
  );
  const upstreams = process.env.AIDASH_E2E_KEYLESS_UPSTREAMS;
  if (!reserved || !upstreams) return `http://127.0.0.1:${port}`;
  // Rename so the proxy never reads a partially written port.
  const published = join(upstreams, String(fixture));
  writeFileSync(`${published}.tmp`, String(port));
  renameSync(`${published}.tmp`, published);
  return `http://127.0.0.1:${reserved}`;
}
