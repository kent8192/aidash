import type { Page } from "@playwright/test";

/** A member of the `acme` Tenant: definitions belong to Tenants (ADR 0029). */
export const tenantSubject = {
  kind: "subject",
  tenant: "acme",
  subject: "alice",
} as const;

/** The administration overview, listing the actions the server computed for
 *  the caller; `assignable` is listed only for a Tenant Administrator. */
export async function routeTenantActions(
  page: Page,
  actions: readonly string[],
  {
    tenant = "acme",
    assignable,
  }: { tenant?: string; assignable?: string[] } = {},
) {
  await page.route(`**/api/tenants/${tenant}/administration`, (route) =>
    route.fulfill({
      json: {
        tenant,
        label: null,
        actions,
        assignable_groups: assignable ?? [],
        policy_revision: assignable ? 1 : null,
      },
    }),
  );
}

/** One `GET /api/registry/versions` item, approved unless overridden. */
export function registryVersion(
  entry: { kind: string },
  overrides: Record<string, unknown> = {},
) {
  return {
    entry,
    needs_approval: true,
    approval: { enabled: true, revision: 1 },
    history: [
      {
        revision: 1,
        enabled: true,
        actor: "alice",
        created_at: "2026-10-17T09:00:00Z",
      },
    ],
    subject: null,
    ...overrides,
  };
}

/** Serves the Tenant's Registry versions; `versions` is read per request. */
export async function routeRegistryVersions(
  page: Page,
  versions: () => unknown[],
) {
  await page.route(
    (url) => url.pathname === "/api/registry/versions",
    (route) => route.fulfill({ json: versions() }),
  );
}
