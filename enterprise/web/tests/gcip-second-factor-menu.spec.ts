import { expect, test, type Page } from "@playwright/test";
import { installBearerDashboard } from "../../../web/tests/auth-fixture";

async function openAccount(
  page: Page,
  provider: string,
  deploymentAdministrator = false,
) {
  await installBearerDashboard(
    page,
    "acceptance-access-token",
    deploymentAdministrator ? undefined : { tenant: "acme", name: "alice" },
  );
  await page.addInitScript(() =>
    localStorage.setItem("aidash-locale", "en-US"),
  );
  await page.route("**/auth/config", (route) =>
    route.fulfill({ json: { enabled: true, provider } }),
  );
  await page.goto("/");
  await expect(page.locator("#main")).toBeVisible();
  await page
    .getByRole("button", { name: "Account settings", exact: true })
    .click();
}
const setup = (page: Page) =>
  page.getByRole("button", { name: "Set up two-step verification" });

test("account settings starts GCIP enrollment through the session's own sign-in", async ({
  page,
}) => {
  await openAccount(page, "gcip");
  await page.route("**/auth/login?**", (route) =>
    route.fulfill({ contentType: "text/plain", body: "sign-in fixture" }),
  );
  const login = page.waitForRequest("**/auth/login?**");
  await setup(page).click();
  const url = new URL((await login).url());
  expect([...url.searchParams.keys()]).toEqual(["return_to"]);
  expect(url.searchParams.get("return_to")).toBe("/");
  // The sign-in page reads this intent after the transaction redirect.
  expect(
    await page.evaluate(() => sessionStorage.getItem("aidash.gcip.enroll")),
  ).not.toBeNull();
});

test("the setup entry is offered only to GCIP sessions", async ({ page }) => {
  await openAccount(page, "oidc");
  await expect(
    page.getByRole("button", { name: "Log out on this device" }),
  ).toBeVisible();
  await expect(setup(page)).toHaveCount(0);
});

test("a bearer Deployment Administrator authority has no GCIP session to enroll through", async ({
  page,
}) => {
  await openAccount(page, "gcip", true);
  await expect(
    page.getByRole("button", { name: "Log out on this device" }),
  ).toBeVisible();
  await expect(setup(page)).toHaveCount(0);
});
