import { Link } from "@tanstack/react-router";
import { Hint, StateBadge } from "../components/patterns";
import { useI18n } from "../ui";
import {
  readinessKindLabel,
  readinessStatusLabel,
  readinessTone,
  secretReadinessCopy,
  type SecretReadinessFact,
} from "./secret-readiness-model";

/** One reusable readiness view: the Agent page and the Marketplace install
 *  screen pass their facts, and #498 can mount the same component. */
export function SecretReadiness({ facts }: { facts: SecretReadinessFact[] }) {
  const { locale } = useI18n();
  const copy = secretReadinessCopy[locale];
  if (!facts.length) return null;
  return (
    <section
      className="grid min-w-0 gap-2"
      aria-label={copy.title}
      aria-live="polite"
    >
      <h3 className="text-[13px] font-medium text-foreground">{copy.title}</h3>
      <Hint>{copy.help}</Hint>
      <ul className="divide-y divide-border rounded-md border border-border">
        {facts.map((fact) => (
          <li
            key={`${fact.kind}:${fact.item}`}
            className="grid min-w-0 gap-1.5 px-3 py-2.5"
          >
            <div className="flex min-w-0 flex-wrap items-center justify-between gap-2">
              <div className="flex min-w-0 flex-wrap items-baseline gap-2">
                <span className="text-[11px] uppercase tracking-wide text-faint">
                  {readinessKindLabel(fact.kind, locale)}
                </span>
                <span className="min-w-0 break-all font-mono text-xs text-foreground">
                  {fact.item}
                </span>
              </div>
              <StateBadge tone={readinessTone(fact.status)}>
                {readinessStatusLabel(fact.status, locale)}
              </StateBadge>
            </div>
            {fact.tenant_secret_name && (
              <p className="text-xs text-muted-foreground">
                {copy.secret} ·{" "}
                <span className="font-mono">{fact.tenant_secret_name}</span>
              </p>
            )}
            {fact.repair === "manage_secret" && (
              <Link
                to="/$section"
                params={{ section: "trust" }}
                search={{ view: "authorization" }}
                className="w-fit text-xs text-brand underline-offset-4 hover:underline focus-visible:outline-2 focus-visible:outline-ring"
              >
                {copy.manage}
              </Link>
            )}
            {fact.repair === "ask_administrator" && (
              <Hint>{copy.askAdministrator}</Hint>
            )}
            {fact.repair === "deployment_administrator" && (
              <p
                className="text-xs leading-relaxed text-destructive"
                role="note"
              >
                {copy.deployment_administrator}
              </p>
            )}
          </li>
        ))}
      </ul>
    </section>
  );
}
