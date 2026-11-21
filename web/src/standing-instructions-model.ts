/** Tenant Instructions and Node Instructions (#1010): the server refuses a
 *  text over this many UTF-8 bytes or containing NUL. */
export const MAX_STANDING_INSTRUCTIONS_BYTES = 16_384;

export type StandingInstructionsProblem = "too_long" | "nul";

/** The UTF-8 length the server measures. */
export function utf8Bytes(text: string): number {
  return new TextEncoder().encode(text).length;
}

/** Why the server would refuse `text`, checked before sending it. */
export function standingInstructionsProblems(
  text: string,
): StandingInstructionsProblem[] {
  const problems: StandingInstructionsProblem[] = [];
  if (utf8Bytes(text) > MAX_STANDING_INSTRUCTIONS_BYTES)
    problems.push("too_long");
  if (text.includes("\0")) problems.push("nul");
  return problems;
}

/** The message that explains each problem, as a locale key. */
export const STANDING_INSTRUCTIONS_PROBLEM_MESSAGES = {
  too_long: "instructionsTooLong",
  nul: "instructionsNul",
} as const satisfies Record<StandingInstructionsProblem, string>;
