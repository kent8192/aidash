import { Bot, Code2, Search, ShieldCheck, Zap, UserRound } from "lucide-react";

/** Icon selection is cosmetic only; authority and roles come from server records. */
export function Avatar({
  name,
  human = false,
  small = false,
}: {
  name: string;
  human?: boolean;
  small?: boolean;
}) {
  const variant = /research/i.test(name)
    ? "research"
    : /cod/i.test(name)
      ? "code"
      : /verif|test/i.test(name)
        ? "verify"
        : /publish/i.test(name)
          ? "publish"
          : "agent";
  const Icon = human
    ? UserRound
    : {
        research: Search,
        code: Code2,
        verify: ShieldCheck,
        publish: Zap,
        agent: Bot,
      }[variant];
  return (
    <span
      className={`workspace-avatar ${human ? "human" : variant} ${small ? "small" : ""}`}
      aria-hidden="true"
    >
      <Icon size={small ? 13 : 18} strokeWidth={1.5} />
    </span>
  );
}
