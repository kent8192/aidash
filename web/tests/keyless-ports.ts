/**
 * Loopback ports the e2e nodes permit for Keyless endpoints (#256). A Keyless
 * model or embedding endpoint reaches a private address only when the
 * Deployment lists its exact origin, so `scripts/golden_path.py` reserves one
 * port per spec below and permits it. Elsewhere any free port is used.
 */
export const KeylessFixture = {
  semantic: 0,
  generation: 1,
  scopedAccess: 2,
} as const;

export function keylessPort(
  fixture: (typeof KeylessFixture)[keyof typeof KeylessFixture],
): number {
  const ports = (process.env.AIDASH_E2E_KEYLESS_PORTS ?? "")
    .split(",")
    .filter(Boolean)
    .map(Number);
  return ports[fixture] ?? 0;
}
