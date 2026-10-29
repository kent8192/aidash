/** The body of a Safety Stop command (ADR 0101). */
export type StopInput = {
  tenant: string;
  action: "pause" | "cancel";
  expected_control_revision: number;
  reason: string;
};

/**
 * What identifies one command for retry. The same signature reuses its
 * operation ID, so a retry is a replay; any change is a new command.
 */
export function safetyStopSignature(run: string, input: StopInput): string {
  return JSON.stringify([
    run,
    input.tenant,
    input.action,
    input.expected_control_revision,
    input.reason,
  ]);
}

/** The Tenant and Run a lookup is for, normalised as the form submits them. */
export function safetyStopTarget(tenant: string, run: string): string {
  return JSON.stringify([tenant.trim(), run.trim()]);
}

/**
 * The looked-up facts, only while the form still names the target they were
 * read for. A control revision belongs to one Run; once the target changes the
 * facts are gone, so a stop can never carry another Run's revision.
 */
export function factsForTarget<T>(
  loaded: { target: string; facts: T } | undefined,
  target: string,
): T | undefined {
  return loaded?.target === target ? loaded.facts : undefined;
}
