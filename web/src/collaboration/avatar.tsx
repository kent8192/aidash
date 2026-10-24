import { cn } from "../lib/utils";

/** Up to `length` display initials; Latin multi-word names use one letter per word. */
export function initials(name: string, length: 1 | 2 = 1) {
  const words = name
    .trim()
    .split(/[\s._@/-]+/)
    .filter(Boolean);
  if (words.length === 0) return "?";
  const latin = /^[\p{Script=Latin}\d]/u.test(words[0]);
  const text =
    length === 2 && latin && words.length > 1
      ? `${[...words[0]][0]}${[...words[1]][0]}`
      : [...words[0]].slice(0, length).join("");
  return text.toUpperCase();
}

/** Rounded-square initials. Appearance is cosmetic; authority and roles come from server records. */
export function Avatar({
  name,
  human = false,
  small = false,
  className,
}: {
  name: string;
  human?: boolean;
  small?: boolean;
  className?: string;
}) {
  return (
    <span
      aria-hidden="true"
      className={cn(
        "inline-grid shrink-0 select-none place-items-center rounded-md font-semibold leading-none ring-1 ring-inset",
        small ? "size-5 text-[10px]" : "size-7 text-xs",
        human
          ? "bg-brand-soft text-brand ring-brand-line"
          : "bg-raised text-foreground ring-border-strong",
        className,
      )}
    >
      {initials(name)}
    </span>
  );
}
