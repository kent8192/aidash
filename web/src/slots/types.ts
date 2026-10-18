import type { ComponentType, LazyExoticComponent } from "react";
import type { Locale } from "@/ui";

/**
 * The contracts of the extension slots. A slot is one module that `@slots/<name>`
 * resolves to. Community ships a stub for each; the Enterprise build points the
 * alias at its own directory, which must export the same names.
 */

export type SignInPageProps = {
  locale: Locale;
  setLocale: (locale: Locale) => void;
};

/** A browser sign-in for one `provider` that `/auth/config` can report. */
export type SignInMethod = {
  Page:
    | ComponentType<SignInPageProps>
    | LazyExoticComponent<ComponentType<SignInPageProps>>;
  /** The button label on the dashboard's connection card. */
  label: Record<Locale, string>;
};

export type AccountEntriesProps = {
  /** The `provider` that `/auth/config` reported. */
  provider: string;
  /** The authority selected in this tab. */
  context: string | null;
  mappings: readonly { id: string; tenant: string }[];
};
