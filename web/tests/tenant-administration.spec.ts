import { expect, test } from "@playwright/test";
import { installBearerDashboard } from "./auth-fixture";

const access = { kind: "subject", tenant: "alpha", subject: "alice" };
const mapping = (
  id: string,
  identity: string,
  subject: string,
  own: { identity?: boolean; subject?: boolean; enabled?: boolean } = {},
) => ({
  id,
  identity: {
    id: identity,
    verified_email: `${identity}@example.test`,
    display_name: identity,
  },
  subject,
  enabled: own.enabled ?? true,
  revision: 1,
  groups: [],
  own_identity: own.identity ?? false,
  own_subject: own.subject ?? false,
});

test("a Tenant Administrator is not offered controls the server marks as its own", async ({
  page,
}) => {
  // The fixture session acts through `fixture-mapping` as `alice`, which is
  // on another page; only the server knows which rows here are its own.
  await installBearerDashboard(page, "fixture", {
    tenant: "alpha",
    name: "alice",
  });
  await page.addInitScript(() =>
    localStorage.setItem("aidash-locale", "en-US"),
  );
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/api/events/stream") return route.abort();
    if (path === "/api/session")
      return route.fulfill({ json: { access, node_id: "aidash://test" } });
    if (path === "/api/state")
      return route.fulfill({
        json: {
          access,
          node: {
            id: "aidash://test",
            endpoint: "http://localhost",
            protocol_version: "0.1",
            capabilities: [],
            clusters: [],
          },
          registry: [],
          workspaces: [],
          tasks: [],
          artifacts: [],
          events: [],
          runs: [],
          conversations: [],
          human_requests: [],
          installations: [],
          peers: [],
        },
      });
    if (path === "/api/mesh")
      return route.fulfill({ json: { nodes: [], errors: [] } });
    if (path === "/api/tenants/alpha/administration")
      return route.fulfill({
        json: {
          tenant: "alpha",
          actions: [
            "registration.read",
            "registration.approve",
            "registration.reject",
            "mapping.read",
            "mapping.disable",
            "subject.group.update",
          ],
          assignable_groups: ["members"],
          policy_revision: 3,
        },
      });
    if (path === "/api/tenants/alpha/registrations")
      return route.fulfill({ json: [] });
    if (path === "/api/tenants/alpha/mappings")
      return route.fulfill({
        json: [
          // A disabled historical Mapping of the caller's External Identity.
          mapping("alice-old-mapping", "identity-alice", "alice-old", {
            identity: true,
            subject: true,
            enabled: false,
          }),
          mapping("alice-ops-mapping", "identity-alice", "alice-ops", {
            identity: true,
            subject: true,
          }),
          // Another person mapped to a subject the caller acts as.
          mapping("shared-mapping", "identity-bob", "shared", {
            subject: true,
          }),
          mapping("carol-mapping", "identity-carol", "carol"),
        ],
      });
    return route.fulfill({ json: [] });
  });
  // `/authorization` would reach Vite's `/auth` proxy instead of the dashboard.
  await page.goto("/trust?view=authorization");
  const members = page.locator("li").filter({ hasText: "→" });
  const row = (subject: string) =>
    members.filter({
      has: page.locator("span.font-mono", {
        hasText: new RegExp(`^${subject}$`),
      }),
    });
  const disable = (subject: string) =>
    row(subject).getByRole("button", { name: "Disable", exact: true });
  await expect(row("carol")).toHaveCount(1);
  await expect(disable("carol")).toBeVisible();
  await expect(row("carol").getByRole("checkbox")).toHaveCount(1);
  await expect(disable("shared")).toBeVisible();
  await expect(row("shared").getByRole("checkbox")).toHaveCount(0);
  for (const own of ["alice-old", "alice-ops"]) {
    await expect(row(own)).toHaveCount(1);
    await expect(disable(own)).toHaveCount(0);
    await expect(row(own).getByRole("checkbox")).toHaveCount(0);
  }
});

test("an approval submits only the proposed groups that are still assignable", async ({
  page,
}) => {
  await installBearerDashboard(page, "fixture", {
    tenant: "alpha",
    name: "alice",
  });
  await page.addInitScript(() =>
    localStorage.setItem("aidash-locale", "en-US"),
  );
  const approvals: unknown[] = [];
  await page.route("**/api/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    if (path === "/api/events/stream") return route.abort();
    if (path === "/api/session")
      return route.fulfill({ json: { access, node_id: "aidash://test" } });
    if (path === "/api/state")
      return route.fulfill({
        json: {
          access,
          node: {
            id: "aidash://test",
            endpoint: "http://localhost",
            protocol_version: "0.1",
            capabilities: [],
            clusters: [],
          },
          registry: [],
          workspaces: [],
          tasks: [],
          artifacts: [],
          events: [],
          runs: [],
          conversations: [],
          human_requests: [],
          installations: [],
          peers: [],
        },
      });
    if (path === "/api/mesh")
      return route.fulfill({ json: { nodes: [], errors: [] } });
    if (path === "/api/tenants/alpha/administration")
      return route.fulfill({
        json: {
          tenant: "alpha",
          actions: ["registration.read", "registration.approve"],
          assignable_groups: ["members"],
          policy_revision: 3,
        },
      });
    // The Invitation proposed `reviewers`, which is no longer assignable.
    if (path === "/api/tenants/alpha/registrations")
      return route.fulfill({
        json: [
          {
            id: "request-dave",
            identity: {
              id: "identity-dave",
              verified_email: "dave@example.test",
              display_name: "dave",
            },
            expires_at: "2026-10-20T00:00:00Z",
            proposed_groups: ["members", "reviewers"],
          },
        ],
      });
    if (path === "/api/tenants/alpha/registrations/request-dave/approve") {
      approvals.push(request.postDataJSON());
      return route.fulfill({ json: {} });
    }
    return route.fulfill({ json: [] });
  });
  await page.goto("/trust?view=authorization");
  await expect(page.getByRole("checkbox", { name: "members" })).toBeChecked();
  await expect(page.getByRole("checkbox", { name: "reviewers" })).toHaveCount(
    0,
  );
  await page.getByLabel("New user subject").fill("dave");
  await page.getByRole("button", { name: "Approve", exact: true }).click();
  await expect
    .poll(() => approvals)
    .toEqual([{ subject: "dave", groups: ["members"] }]);
});
