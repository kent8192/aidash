/** A generated agent without a per-agent token limit records 10^15
 * (`UNBOUNDED_TOKENS` on the server); show that as no limit. */
export const UNBOUNDED_TOKENS = 1_000_000_000_000_000;
export function tokenLimitLabel(
  limit: number,
  locale: string | undefined,
  noLimit: string,
): string {
  return limit >= UNBOUNDED_TOKENS ? noLimit : limit.toLocaleString(locale);
}
