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

test("a Deployment Administrator invites a Tenant's first administrator and approves what each Invitation proposed", async ({
  page,
}) => {
  await installBearerDashboard(page, "fixture");
  await page.addInitScript(() =>
    localStorage.setItem("aidash-locale", "en-US"),
  );
  // This Deployment issues Invitations.
  await page.route("**/auth/session", (route) =>
    route.fulfill({
      json: {
        id: "fixture-browser-session",
        deployment_administrator: true,
        mappings: [],
        invitations: true,
      },
    }),
  );
  const administrator = { kind: "deployment_administrator" };
  const request = (id: string, invitation: string, groups: string[]) => ({
    id,
    identity_id: `identity-${id}`,
    status: "pending",
    created_at: "2026-10-17T00:00:00Z",
    expires_at: "2026-10-24T00:00:00Z",
    decided_at: null,
    tenant: "beta",
    invitation_id: invitation,
    proposed_groups: groups,
  });
  const issued: unknown[] = [];
  const approvals: unknown[] = [];
  await page.route("**/api/**", async (route) => {
    const call = route.request();
    const path = new URL(call.url()).pathname;
    if (path === "/api/events/stream") return route.abort();
    if (path === "/api/session")
      return route.fulfill({
        json: { access: administrator, node_id: "aidash://test" },
      });
    if (path === "/api/state")
      return route.fulfill({
        json: {
          access: administrator,
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
    // A Tenant created by a Deployment Administrator: nobody is in it yet.
    if (path === "/api/authorization/beta")
      return route.fulfill({
        json: {
          revision: 1,
          bundle: {
            tenant: "beta",
            groups: {
              administrators: { roles: ["administrator"], assignable: true },
              members: { roles: ["member"], assignable: true },
              auditors: { roles: ["auditor"] },
            },
            roles: {},
            subjects: {},
            policies: [],
          },
        },
      });
    if (path === "/api/tenants/beta/invitations" && call.method() === "POST") {
      issued.push(call.postDataJSON());
      return route.fulfill({
        status: 201,
        json: {
          id: "invitation-erin",
          tenant: "beta",
          link: "http://127.0.0.1/invitations/beta#erin-secret",
          secret: "erin-secret",
          groups: ["administrators"],
          expires_at: "2026-10-24T00:00:00Z",
        },
      });
    }
    if (path === "/api/dashboard/registrations")
      return route.fulfill({
        json: [
          request("erin", "invitation-erin", ["administrators"]),
          request("frank", "invitation-frank", ["members"]),
          // A request without an Invitation proposes nothing.
          { ...request("grace", "", []), invitation_id: null },
        ],
      });
    if (path === "/api/dashboard/registrations/frank/approve") {
      approvals.push(call.postDataJSON());
      return route.fulfill({ json: {} });
    }
    return route.fulfill({ json: [] });
  });
  await page.goto("/trust?view=authorization");

  // The queue starts each approval from what its Invitation proposed.
  const queued = (id: string) =>
    page.locator("li").filter({ hasText: `identity-${id}` });
  const groups = "Groups, if the subject is new (comma-separated)";
  await expect(queued("erin").getByLabel(groups)).toHaveValue("administrators");
  await expect(queued("frank").getByLabel(groups)).toHaveValue("members");
  await expect(queued("grace").getByLabel(groups)).toHaveValue("");
  await queued("frank").getByLabel("User subject").fill("frank");
  await queued("frank")
    .getByRole("button", { name: "Approve mapping", exact: true })
    .click();
  await expect
    .poll(() => approvals)
    .toEqual([{ tenant: "beta", subject: "frank", groups: ["members"] }]);

  // The Tenant's view issues Invitations into its Assignable Groups only.
  const open = page
    .locator("form")
    .filter({ has: page.locator('input[name="tenant"]') });
  await open.locator('input[name="tenant"]').fill("beta");
  await open.getByRole("button", { name: "Open", exact: true }).click();
  await expect(page.getByRole("checkbox", { name: "members" })).toBeChecked();
  await expect(page.getByRole("checkbox", { name: "auditors" })).toHaveCount(0);
  await page.getByRole("checkbox", { name: "administrators" }).check();
  await page.getByRole("checkbox", { name: "members" }).uncheck();
  await page
    .getByRole("button", { name: "Create invitation link", exact: true })
    .click();
  await expect(
    page.getByText("http://127.0.0.1/invitations/beta#erin-secret"),
  ).toBeVisible();
  expect(issued).toEqual([
    { groups: ["administrators"], expires_in_seconds: 604_800 },
  ]);
});

test("deleting the Tenant needs its name typed, and leaving asks to confirm", async ({
  page,
}) => {
  await installBearerDashboard(page, "fixture", {
    tenant: "alpha",
    name: "alice",
  });
  await page.addInitScript(() =>
    localStorage.setItem("aidash-locale", "en-US"),
  );
  const deletions: unknown[] = [];
  const leaves: string[] = [];
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
          actions: ["mapping.read", "tenant.delete"],
          assignable_groups: ["members"],
          policy_revision: 3,
        },
      });
    if (path === "/api/tenants/alpha/deletion") {
      deletions.push(request.postDataJSON());
      return route.fulfill({
        json: {
          tenant: "alpha",
          deleted_at: "2026-10-17T00:00:00Z",
          deleted_by_kind: "tenant_administrator",
          purge_after: "2026-11-16T00:00:00Z",
          purged_at: null,
        },
      });
    }
    if (path === "/api/tenants/alpha/leave") {
      leaves.push(request.method());
      return route.fulfill({
        status: 409,
        json: {
          error:
            "appoint another Tenant Administrator or delete the Tenant before leaving",
        },
      });
    }
    return route.fulfill({ json: [] });
  });
  await page.goto("/trust?view=authorization");

  const remove = page.getByRole("button", { name: "Delete Tenant" });
  await expect(remove).toBeDisabled();
  await page.getByLabel("Type the Tenant name alpha to confirm").fill("Alpha");
  await expect(remove).toBeDisabled();
  await page.getByLabel("Type the Tenant name alpha to confirm").fill("alpha");
  await remove.click();
  await expect.poll(() => deletions).toEqual([{ confirmation: "alpha" }]);

  await page.getByRole("button", { name: "Leave Tenant" }).click();
  expect(leaves).toEqual([]);
  await page.getByRole("button", { name: "Leave now" }).click();
  await expect(
    page.getByText("You are the last Tenant Administrator."),
  ).toBeVisible();
  expect(leaves).toEqual(["POST"]);
});

test("a Deployment Administrator restores what the server marks restorable and leaves a Tenant it deleted", async ({
  page,
}) => {
  await installBearerDashboard(page, "fixture");
  await page.addInitScript(() =>
    localStorage.setItem("aidash-locale", "en-US"),
  );
  await page.route("**/auth/session", (route) =>
    route.fulfill({
      json: {
        id: "fixture-browser-session",
        deployment_administrator: true,
        mappings: [],
        invitations: false,
      },
    }),
  );
  const administrator = { kind: "deployment_administrator" };
  const deletion = (tenant: string) => ({
    tenant,
    deleted_at: "2026-10-17T00:00:00Z",
    deleted_by_kind: "deployment_administrator",
    purge_after: "2026-11-16T00:00:00Z",
    purged_at: null,
  });
  const record = (name: string, restorable: boolean) => ({
    name,
    label: null,
    personal: false,
    created_at: "2026-10-01T00:00:00Z",
    created_by_kind: "deployment_administrator",
    deletion: deletion(name),
    restorable,
  });
  let policyReads = 0;
  await page.route("**/api/**", async (route) => {
    const call = route.request();
    const path = new URL(call.url()).pathname;
    if (path === "/api/events/stream") return route.abort();
    if (path === "/api/session")
      return route.fulfill({
        json: { access: administrator, node_id: "aidash://test" },
      });
    if (path === "/api/state")
      return route.fulfill({
        json: {
          access: administrator,
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
    if (path === "/api/tenants")
      // The server's clock decides; whatever this device's clock says.
      return route.fulfill({
        json: [record("open-otter", true), record("late-lark", false)],
      });
    if (path === "/api/authorization/beta") {
      policyReads += 1;
      return route.fulfill({
        json: {
          revision: 1,
          bundle: {
            tenant: "beta",
            groups: {},
            roles: {},
            subjects: {},
            policies: [],
          },
        },
      });
    }
    if (path === "/api/tenants/beta/deletion")
      return route.fulfill({ json: deletion("beta") });
    return route.fulfill({ json: [] });
  });
  await page.goto("/trust?view=authorization");

  const listed = (name: string) => page.locator("li").filter({ hasText: name });
  await expect(listed("open-otter")).toContainText("Deleted; restorable until");
  await expect(
    listed("open-otter").getByRole("button", { name: "Restore" }),
  ).toHaveCount(1);
  await expect(listed("late-lark")).toContainText("the restore deadline");
  await expect(
    listed("late-lark").getByRole("button", { name: "Restore" }),
  ).toHaveCount(0);

  // Deleting the open Tenant closes its view, so its policy is not polled.
  const open = page
    .locator("form")
    .filter({ has: page.locator('input[name="tenant"]') });
  await open.locator('input[name="tenant"]').fill("beta");
  await open.getByRole("button", { name: "Open", exact: true }).click();
  await page.getByLabel("Type the Tenant name beta to confirm").fill("beta");
  await page.getByRole("button", { name: "Delete Tenant" }).click();
  await expect(page.getByRole("button", { name: "Delete Tenant" })).toHaveCount(
    0,
  );
  const reads = policyReads;
  await page.waitForTimeout(6_000);
  expect(policyReads).toBe(reads);

test("a member issues its own API token, and a policy reader simulates without recording (#363)", async ({
  page,
}) => {
  await installBearerDashboard(page, "fixture", {
    tenant: "alpha",
    name: "alice",
  });
  await page.addInitScript(() =>
    localStorage.setItem("aidash-locale", "en-US"),
  );
  const issued: unknown[] = [];
  await page.route("**/api/**", async (route) => {
    const url = new URL(route.request().url());
    const path = url.pathname;
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
          actions: ["policy.read", "credential.create"],
          assignable_groups: ["members"],
          policy_revision: 3,
        },
      });
    if (path === "/api/authorization/alpha")
      return route.fulfill({
        json: {
          revision: 3,
          bundle: {
            tenant: "alpha",
            subjects: { alice: { kind: "user", groups: ["members"] } },
            groups: { members: { roles: [], assignable: true } },
            roles: {},
            policies: [],
          },
        },
      });
    if (path === "/api/authorization/alpha/credentials") {
      if (route.request().method() === "POST") {
        issued.push(route.request().postDataJSON());
        return route.fulfill({
          json: {
            credential: {
              id: "00000000-0000-0000-0000-000000000009",
              tenant: "alpha",
              subject: "alice",
              created_at: "2026-10-18T00:00:00Z",
              expires_at: "2026-10-18T01:00:00Z",
              revoked_at: null,
              issued_by: "oidc:identity-alice",
              mapping_id: "fixture-mapping",
            },
            token: "aidash_subject_alpha_fixture",
          },
        });
      }
      // Without `credential.read`, a member lists only its own tokens.
      expect(url.searchParams.get("mine")).toBe("true");
      return route.fulfill({ json: [] });
    }
    return route.fulfill({ json: [] });
  });
  await page.goto("/trust?view=authorization");
  await expect(page.getByText("Your API tokens")).toBeVisible();
  await page
    .getByRole("button", { name: "Issue credential", exact: true })
    .first()
    .click();
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("button", { name: "Issue credential" }).click();
  await expect(page.getByRole("dialog").getByRole("textbox")).toHaveValue(
    "aidash_subject_alpha_fixture",
  );
  expect(issued).toEqual([{ subject: "alice", expires_in_seconds: 3600 }]);
  await page.getByRole("button", { name: "I have saved the token" }).click();
  // The policy is readable, but neither editable nor recordable here.
  await expect(page.getByText("Policy bundle")).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Edit access policy" }),
  ).toHaveCount(0);
  await expect(
    page.getByText("Save this decision to the audit history"),
  ).toHaveCount(0);
});

test("a policy reader allowed to issue credentials but not to list them can still issue one (#363)", async ({
  page,
}) => {
  await installBearerDashboard(page, "fixture", {
    tenant: "alpha",
    name: "alice",
  });
  await page.addInitScript(() =>
    localStorage.setItem("aidash-locale", "en-US"),
  );
  const listed: string[] = [];
  await page.route("**/api/**", async (route) => {
    const url = new URL(route.request().url());
    const path = url.pathname;
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
          actions: ["policy.read", "credential.issue"],
          assignable_groups: [],
          policy_revision: 3,
        },
      });
    if (path === "/api/authorization/alpha")
      return route.fulfill({
        json: {
          revision: 3,
          bundle: {
            tenant: "alpha",
            subjects: { alice: { kind: "user", groups: [] } },
            groups: {},
            roles: {},
            policies: [],
          },
        },
      });
    if (path === "/api/authorization/alpha/credentials") {
      if (url.searchParams.get("mine") !== "true") listed.push(url.search);
      return route.fulfill({ json: [] });
    }
    return route.fulfill({ json: [] });
  });
  await page.goto("/trust?view=authorization");
  await expect(
    page.getByRole("button", { name: "Issue credential", exact: true }),
  ).toHaveCount(1);
  await expect(page.getByText("Subject credentials")).toBeVisible();
  // The Tenant's credential list stays unread without `credential.read`.
  await page.waitForTimeout(1_000);
  expect(listed).toEqual([]);
});
