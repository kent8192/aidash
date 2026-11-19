/** Budgets (#1012): limits are stored in billionths of a credit (one credit
 *  is one US dollar on OpenRouter); people type credits. */
const SCALE = 9;

/** Credits typed as text ("12.5") to nanocredits, without binary floating
 *  point; undefined for an empty field, null for text that is no amount. */
export function creditsToNanocredits(text: string): number | null | undefined {
  const value = text.trim();
  if (value === "") return undefined;
  const match = /^(\d+)(?:\.(\d{1,9}))?$/.exec(value);
  if (!match) return null;
  const whole = BigInt(match[1]);
  const fraction = BigInt((match[2] ?? "").padEnd(SCALE, "0"));
  const nanocredits = whole * 10n ** BigInt(SCALE) + fraction;
  if (nanocredits <= 0n || nanocredits > BigInt(Number.MAX_SAFE_INTEGER))
    return null;
  return Number(nanocredits);
}

/** Nanocredits as credits with two decimals, for display. */
export function formatCredits(nanocredits: number): string {
  return (nanocredits / 1e9).toFixed(2);
}

/** Recipients typed one per line or separated by commas. */
export function parseRecipients(text: string): string[] {
  return text
    .split(/[\n,]/)
    .map((address) => address.trim())
    .filter((address) => address !== "");
}

export type BudgetProblem =
  | "no_limit"
  | "bad_amount"
  | "soft_not_below_hard"
  | "no_recipient"
  | "too_many_recipients"
  | "bad_address";

/** Why the server would refuse a Budget, checked before sending it. */
export function budgetProblems(input: {
  soft: string;
  hard: string;
  recipients: string;
}): BudgetProblem[] {
  const problems: BudgetProblem[] = [];
  const soft = creditsToNanocredits(input.soft);
  const hard = creditsToNanocredits(input.hard);
  const recipients = parseRecipients(input.recipients);
  if (soft === null || hard === null) problems.push("bad_amount");
  if (soft === undefined && hard === undefined) problems.push("no_limit");
  if (typeof soft === "number" && typeof hard === "number" && soft >= hard)
    problems.push("soft_not_below_hard");
  if (typeof soft === "number" && recipients.length === 0)
    problems.push("no_recipient");
  if (recipients.length > 20) problems.push("too_many_recipients");
  if (recipients.some((address) => !/^[^@\s]+@[^@\s]+$/.test(address)))
    problems.push("bad_address");
  return problems;
}
