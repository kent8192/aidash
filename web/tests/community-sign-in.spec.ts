import { expect, test } from "@playwright/test";
import { installBearerDashboard } from "./auth-fixture";

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() =>
    localStorage.setItem("aidash-locale", "en-US"),
  );
});

// The Enterprise slots are what give a provider its own page and account entries.
test("Community has no sign-in page for a GCIP provider and loads no GCIP code", async ({
  page,
}) => {
  const requested: string[] = [];
  page.on("request", (request) => requested.push(request.url()));
  await page.route("**/auth/config", (route) =>
    route.fulfill({ json: { enabled: true, provider: "gcip" } }),
  );
  await page.goto("/sign-in");
  await expect(
    page.getByText("Ask an administrator to configure OIDC for Aidash."),
  ).toBeVisible();
  expect(requested.filter((url) => /gcip|firebase/i.test(url))).toEqual([]);
});

test("Community's account menu has no two-step verification entry", async ({
  page,
}) => {
  await installBearerDashboard(page, "acceptance-access-token", {
    tenant: "acme",
    name: "alice",
  });
  await page.route("**/auth/config", (route) =>
    route.fulfill({ json: { enabled: true, provider: "gcip" } }),
  );
  await page.goto("/");
  await page
    .getByRole("button", { name: "Account settings", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "Log out on this device" }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Set up two-step verification" }),
  ).toHaveCount(0);
});
