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
