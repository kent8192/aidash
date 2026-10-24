import { useId, type ReactNode } from "react";
import { Languages, LogOut, Server, SunMoon, UserRoundCog } from "lucide-react";
import { initials } from "../collaboration/avatar";
import { NativeSelect } from "../components/ui/native-select";
import { Button } from "../components/ui/button";
import { cn } from "../lib/utils";
import { mappingContext } from "../transport";
import type { ThemePreference } from "../theme";
import { useI18n, type Locale } from "../ui";
import { authCopy, shellCopy } from "./copy";
import { CreateTenantForm, type CreatedTenant } from "../tenancy";
import { DeletedTenants, type DeletedTenant } from "../tenant-deletion";
import { useNodes } from "../nodes";
import { AccountEntries } from "@slots/account-menu";

export type Account = {
  name: string;
  /** The `provider` that `/auth/config` reported. */
  provider: string;
  context: string | null;
  mappings: {
    id: string;
    tenant: string;
    label?: string | null;
    subject: string;
    personal?: boolean;
  }[];
  deploymentAdministratorAllowed: boolean;
  /** Whether the Tenancy Rules let this identity create Tenants now. */
  tenantCreation: boolean;
  tenantCreated: (created: CreatedTenant) => void;
  /** A direct Google account may request shared access even after its personal Mapping is disabled. */
  personalAccount: boolean;
  /** Deleted Tenants this identity is mapped into, with Restore where allowed. */
  deletedTenants: DeletedTenant[];
  /** Whether this identity's Personal Tenant is deleted and it may start a new one. */
  personalTenantDeleted: boolean;
  sharedAccess: {
    status?: string;
    loading: boolean;
    submitting: boolean;
    error?: string;
    request: () => void;
  };
  chooseContext: (context: string) => void;
  /** Works in another Node of the Tenant. */
  chooseNode: (name: string) => void;
  setLocale: (locale: Locale) => void;
  logOut: (allDevices: boolean) => void;
  preference: ThemePreference;
  setPreference: (preference: ThemePreference) => void;
};

const preferences = ["dark", "light", "system"] as const;

export function AccountAvatar({
  name,
  large = false,
}: {
  name: string;
  large?: boolean;
}) {
  return (
    <span
      aria-hidden
      className={cn(
        "grid shrink-0 place-items-center rounded-md bg-raised font-semibold tracking-wide text-foreground ring-1 ring-inset ring-border-strong",
        large ? "size-10 text-[13px]" : "size-[34px] text-[11.5px]",
      )}
    >
      {initials(name, 2)}
    </span>
  );
}

function Group({
  label,
  icon: Icon,
  children,
}: {
  label: string;
  icon: typeof Languages;
  children: ReactNode;
}) {
  return (
    <div className="grid gap-1.5 border-t border-border px-2 py-2.5">
      <span className="flex items-center gap-1.5 text-[11px] text-faint">
        <Icon aria-hidden className="size-3.5" />
        {label}
      </span>
      {children}
    </div>
  );
}

/** Account menu body: authority, language, theme preference and log-out. */
export function AccountPanel({ account }: { account: Account }) {
  const { locale, t } = useI18n();
  const copy = shellCopy[locale];
  const auth = authCopy[locale];
  const shared = account.sharedAccess;
  const personalMapping = account.mappings.some((mapping) => mapping.personal);
  const themeGroup = useId();
  const { nodes, selected } = useNodes();
  return (
    <div className="grid text-[13px]">
      <div className="flex items-center gap-2.5 px-2 pb-3 pt-1.5">
        <AccountAvatar name={account.name} large />
        <span className="min-w-0">
          <span className="block truncate font-medium">{account.name}</span>
          <span className="block truncate font-mono text-[11px] text-faint">
            {account.context === "deployment_administrator"
              ? auth.deploymentAdministrator
              : account.mappings
                  .filter(
                    (mapping) => account.context === mappingContext(mapping),
                  )
                  .map(
                    (mapping) =>
                      `${mapping.label ?? mapping.tenant} / ${mapping.subject}`,
                  )}
          </span>
        </span>
      </div>
      <Group label={copy.authority} icon={UserRoundCog}>
        <label>
          <span className="sr-only">{auth.choose}</span>
          <NativeSelect
            value={account.context ?? ""}
            onChange={(event) => account.chooseContext(event.target.value)}
            className="font-mono text-xs"
          >
            {account.mappings.map((mapping) => (
              <option key={mapping.id} value={mappingContext(mapping)}>
                {mapping.label ?? mapping.tenant} / {mapping.subject}
              </option>
            ))}
            {account.deploymentAdministratorAllowed && (
              <option value="deployment_administrator">
                {auth.deploymentAdministrator}
              </option>
            )}
          </NativeSelect>
        </label>
        {account.tenantCreation && (
          <div className="pt-1">
            <CreateTenantForm onCreated={account.tenantCreated} />
          </div>
        )}
        <div className="pt-1">
          <DeletedTenants
            tenants={account.deletedTenants}
            personalTenantDeleted={account.personalTenantDeleted}
          />
        </div>
        {account.personalAccount && (
          <div className="grid gap-2 pt-1">
            <p
              className="text-xs leading-relaxed text-muted-foreground"
              aria-live="polite"
            >
              {shared.status === "pending"
                ? personalMapping
                  ? auth.personalPending
                  : auth.pending
                : shared.status === "rejected"
                  ? auth.rejected
                  : shared.status === "expired"
                    ? auth.expired
                    : shared.status === "approved"
                      ? auth.sharedApproved
                      : auth.sharedHelp}
            </p>
            {shared.error && (
              <p role="alert" className="text-xs text-destructive">
                {shared.error}
              </p>
            )}
            {shared.status !== "pending" && (
              <Button
                variant="outline"
                size="sm"
                type="button"
                disabled={shared.loading || shared.submitting}
                onClick={shared.request}
              >
                {auth.sharedRequest}
              </Button>
            )}
          </div>
        )}
      </Group>
      {nodes.length > 0 && (
        <Group label={copy.node} icon={Server}>
          <label>
            <span className="sr-only">{copy.node}</span>
            <NativeSelect
              data-testid="node-selector"
              value={selected?.name ?? ""}
              onChange={(event) => account.chooseNode(event.target.value)}
              className="text-xs"
            >
              {nodes.map((node) => (
                <option key={node.name} value={node.name}>
                  {node.label}
                  {node.default ? ` (${copy.nodeDefault})` : ""}
                </option>
              ))}
            </NativeSelect>
          </label>
        </Group>
      )}
      <Group label={copy.language} icon={Languages}>
        <label>
          <span className="sr-only">{t("language")}</span>
          <NativeSelect
            data-testid="language-selector"
            value={locale}
            onChange={(event) =>
              account.setLocale(event.target.value as Locale)
            }
          >
            <option value="ja-JP">日本語</option>
            <option value="en-US">English</option>
          </NativeSelect>
        </label>
      </Group>
      <Group label={copy.theme} icon={SunMoon}>
        <fieldset className="inline-flex gap-0.5 self-start rounded-md border border-border bg-background p-0.5">
          <legend className="sr-only">{copy.theme}</legend>
          {preferences.map((value) => (
            <label
              key={value}
              className="relative inline-flex h-6 cursor-pointer items-center whitespace-nowrap rounded-sm px-2.5 text-xs text-muted-foreground transition-colors duration-150 hover:text-foreground has-[:checked]:bg-raised has-[:checked]:text-foreground has-[:checked]:ring-1 has-[:checked]:ring-inset has-[:checked]:ring-border-strong has-[:focus-visible]:outline-2 has-[:focus-visible]:outline-ring"
            >
              <input
                type="radio"
                name={themeGroup}
                value={value}
                checked={account.preference === value}
                onChange={() => account.setPreference(value)}
                className="absolute inset-0 m-0 cursor-pointer appearance-none rounded-sm opacity-0"
              />
              {copy.themes[value]}
            </label>
          ))}
        </fieldset>
      </Group>
      <AccountEntries
        provider={account.provider}
        context={account.context}
        mappings={account.mappings}
      />
      <div className="grid gap-px border-t border-border pt-1.5">
        {[false, true].map((allDevices) => (
          <button
            key={String(allDevices)}
            type="button"
            onClick={() => account.logOut(allDevices)}
            className="flex h-8 items-center gap-2.5 rounded-md px-2 text-left text-muted-foreground transition-colors duration-150 hover:bg-accent hover:text-foreground"
          >
            <LogOut aria-hidden className="size-3.5" />
            {allDevices ? auth.allDevices : auth.currentDevice}
          </button>
        ))}
      </div>
    </div>
  );
}
