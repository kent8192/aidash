import { expect, test, type Page } from "@playwright/test";

const tenant = "brave-otter-k3m9q2xa";

async function signedIn(page: Page, session: Record<string, unknown> = {}) {
  await page.addInitScript(() => {
    localStorage.setItem("aidash-locale", "en-US");
    document.cookie = "aidash-csrf=tenancy-csrf; path=/";
  });
  await page.route("**/auth/config", (route) =>
    route.fulfill({ json: { enabled: true, login_url: "/auth/login" } }),
  );
  await page.route("**/auth/session", (route) =>
    route.fulfill({
      json: {
        id: "tenancy-session",
        deployment_administrator: false,
        mappings: [],
        tenant_creation: false,
        invitations: true,
        ...session,
      },
    }),
  );
}

test("an Invitation shows its Tenant without spending the link, then requests to join", async ({
  page,
}) => {
  await signedIn(page);
  const inspected: unknown[] = [];
  const redeemed: unknown[] = [];
  await page.route("**/auth/invitations/inspect", (route) => {
    inspected.push(route.request().postDataJSON());
    return route.fulfill({ json: { tenant, label: "Research" } });
  });
  await page.route("**/auth/invitations/redeem", (route) => {
    redeemed.push(route.request().postDataJSON());
    return route.fulfill({
      json: {
        id: "request",
        identity_id: "identity",
        status: "pending",
        created_at: "2026-10-16T00:00:00Z",
        expires_at: "2026-10-23T00:00:00Z",
        decided_at: null,
        tenant,
      },
    });
  });

  await page.goto(`/invitations/${tenant}#link-secret`);

  await expect(page.getByText("Research")).toBeVisible();
  // The secret leaves the address bar; it never reaches a server log.
  expect(new URL(page.url()).hash).toBe("");
  expect(inspected).toEqual([{ tenant, secret: "link-secret" }]);
  expect(redeemed).toEqual([]);
  await page.getByRole("button", { name: "Request to join" }).click();
  await expect(page.getByText("Your request was sent")).toBeVisible();
  expect(redeemed).toEqual([{ tenant, secret: "link-secret" }]);
});

test("an unusable Invitation link says so without telling why", async ({
  page,
}) => {
  await signedIn(page);
  await page.route("**/auth/invitations/inspect", (route) =>
    route.fulfill({ status: 404, json: { error: "invalid link" } }),
  );

  await page.goto(`/invitations/${tenant}#spent-secret`);

  await expect(
    page.getByText("This invitation link is invalid, expired or already used"),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Request to join" }),
  ).toHaveCount(0);
});

test("a signed-out holder is asked to sign in and keeps the link for after", async ({
  page,
}) => {
  await page.addInitScript(() =>
    localStorage.setItem("aidash-locale", "en-US"),
  );
  await page.route("**/auth/session", (route) =>
    route.fulfill({ status: 401, json: { error: "unauthorized" } }),
  );
  let login = "";
  await page.route("**/auth/login?**", (route) => {
    login = route.request().url();
    return route.fulfill({ status: 200, body: "signing in" });
  });

  await page.goto(`/invitations/${tenant}#held-secret`);
  await page.getByRole("button", { name: "Sign in" }).click();

  await expect.poll(() => login).toContain("return_to=%2Finvitations%2F");
  expect(login).not.toContain("held-secret");
  expect(
    await page.evaluate(() => sessionStorage.getItem("aidash-invitation")),
  ).toContain("held-secret");
});

test("a person without a Mapping can create a Tenant where the rules allow it", async ({
  page,
}) => {
  await signedIn(page, { tenant_creation: true });
  await page.route("**/auth/registration", (route) =>
    route.fulfill({ json: null }),
  );
  const created: unknown[] = [];
  await page.route("**/api/tenants", (route) => {
    created.push(route.request().postDataJSON());
    return route.fulfill({
      status: 403,
      json: { error: "tenant_limit_reached" },
    });
  });

  await page.goto("/");
  await page.getByLabel("New Tenant name").fill("Research");
  await page.getByRole("button", { name: "Create Tenant" }).click();

  await expect(
    page.getByText("You have reached the number of Tenants you can create."),
  ).toBeVisible();
  expect(created).toEqual([{ label: "Research" }]);
});

test("a deleted Tenant is offered for restore through its own Mapping", async ({
  page,
}) => {
  await signedIn(page, {
    deleted_tenants: [
      {
        tenant,
        label: "Research",
        mapping_id: "mapping-research",
        personal: false,
        purge_after: "2026-11-16T00:00:00Z",
        restorable: true,
      },
      {
        tenant: "calm-heron-p2s8d1qe",
        label: "Shared",
        mapping_id: "mapping-shared",
        personal: false,
        purge_after: "2026-11-16T00:00:00Z",
        restorable: false,
      },
    ],
  });
  await page.route("**/auth/registration", (route) =>
    route.fulfill({ json: null }),
  );
  const contexts: (string | null)[] = [];
  await page.route(`**/api/tenants/${tenant}/restore`, (route) => {
    contexts.push(route.request().headers()["x-aidash-context"] ?? null);
    return route.fulfill({ json: { tenant } });
  });

  await page.goto("/");

  await expect(page.getByText("Deleted Tenants")).toBeVisible();
  await expect(page.getByText("Shared")).toBeVisible();
  // Only the Tenant this Mapping may restore offers Restore.
  await expect(page.getByRole("button", { name: "Restore" })).toHaveCount(1);
  await page.getByRole("button", { name: "Restore" }).click();
  await expect
    .poll(() => contexts)
    .toEqual([`mapping:${tenant}:mapping-research`]);
});

test("a deleted Personal Tenant can be replaced by a new one on request", async ({
  page,
}) => {
  await signedIn(page, {
    personal_account: true,
    personal_tenant_deleted: true,
    deleted_tenants: [
      {
        tenant,
        label: "Personal",
        mapping_id: "mapping-personal",
        personal: true,
        purge_after: "2026-11-16T00:00:00Z",
        restorable: true,
      },
    ],
  });
  await page.route("**/auth/registration", (route) =>
    route.fulfill({ json: null }),
  );
  let created = 0;
  await page.route("**/api/tenants/personal", (route) => {
    created += 1;
    return route.fulfill({
      status: 201,
      json: {
        name: "calm-heron-p2s8d1qe",
        label: "Personal",
        created_at: "2026-10-17T00:00:00Z",
        mapping_id: "mapping-new",
      },
    });
  });

  await page.goto("/");

  await expect(
    page.getByText("Restore it above to keep its data"),
  ).toBeVisible();
  await page.getByRole("button", { name: "New Personal Tenant" }).click();
  await expect.poll(() => created).toBe(1);
});
