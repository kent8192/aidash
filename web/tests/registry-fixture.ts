import type { Page } from "@playwright/test";

export const tenantSubject = {
  kind: "subject",
  tenant: "acme",
  subject: "alice",
} as const;

/** The Node a Tenant starts with: Catalog Approvals are decided on it. */
export const defaultNode = {
  name: "default-node",
  label: "Default",
  default: true,
  created_at: "2026-10-17T09:00:00Z",
  state: "live",
  deletion: null,
  storage_retained_until_purge: false,
  retained_storage_bytes: null,
} as const;

export async function routeTenantActions(
  page: Page,
  actions: readonly string[],
  {
    tenant = "acme",
    assignable,
    deployment = "aidash://node",
  }: { tenant?: string; assignable?: string[]; deployment?: string } = {},
) {
  // The address is the Deployment of the State the spec serves, which names it
  // in `node.id` once a Node is selected.
  await page.route(`**/api/tenants/${tenant}/nodes`, (route) =>
    route.fulfill({
      json: [{ ...defaultNode, address: `${deployment}/${defaultNode.name}` }],
    }),
  );
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

export async function routeRegistryVersions(
  page: Page,
  versions: () => unknown[],
) {
  await page.route(
    (url) => url.pathname === "/api/registry/versions",
    (route) =>
      route.fulfill({ json: { versions: versions(), next_offset: null } }),
  );
}

/** What `GET /api/tenants/{tenant}/approval-decider` serves a Tenant that has
 *  named no approval Decider: the built-in one, approved on this Node. */
export const builtInApprovalDecider = {
  decider: { id: "aidash.approval-decider", version: "1.0.0" },
  built_in: true,
  registered: true,
  approved: true,
  revision: 0,
};

/** What `GET /api/tenants/{tenant}/org-chart` serves a Tenant whose chains
 *  created nothing yet (#1012). The Trust page and the Thread dashboard both
 *  show the org chart, so a spec that answers unknown paths with `[]` serves
 *  this instead. */
export const emptyOrgChart = {
  since: "2026-11-01T00:00:00Z",
  nodes: [],
  people: [],
};

/** What `GET /api/threads/{thread}/approval-mode` serves a person who set no
 *  mode of their own for the Thread. Every Thread composer shows the switcher
 *  (#1008), so a spec that answers unknown paths with `[]` serves this
 *  instead. */
export const defaultThreadApprovalMode = {
  mode: null,
  default_mode: "manual",
  modes: [
    { mode: "manual", allowed: true, reason: null },
    { mode: "auto", allowed: false, reason: "policy" },
    { mode: "dont_ask", allowed: true, reason: null },
    { mode: "bypass", allowed: true, reason: "not_accepted" },
  ],
};
