import { expect, test } from "@playwright/test";
import { installBearerDashboard } from "./auth-fixture";

const access = { kind: "subject", tenant: "alpha", subject: "alice" };
const mapping = (
  id: string,
  identity: string,
  subject: string,
  groups: string[],
) => ({
  id,
  identity: {
    id: identity,
    verified_email: `${subject}@example.test`,
    display_name: subject,
  },
  subject,
  enabled: true,
  revision: 1,
  groups,
});

test("a Tenant Administrator is not offered controls for its own Mappings", async ({
  page,
}) => {
  // The fixture session acts through `fixture-mapping` as `alice`.
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
          mapping("fixture-mapping", "identity-alice", "alice", []),
          // Another subject the same External Identity is mapped to.
          mapping("alice-ops-mapping", "identity-alice", "alice-ops", []),
          mapping("carol-mapping", "identity-carol", "carol", ["members"]),
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
  await expect(row("carol")).toHaveCount(1);
  await expect(
    row("carol").getByRole("button", { name: "Disable", exact: true }),
  ).toBeVisible();
  await expect(row("carol").getByRole("checkbox")).toHaveCount(1);
  for (const own of ["alice", "alice-ops"]) {
    await expect(row(own)).toHaveCount(1);
    await expect(
      row(own).getByRole("button", { name: "Disable", exact: true }),
    ).toHaveCount(0);
    await expect(row(own).getByRole("checkbox")).toHaveCount(0);
  }
});
