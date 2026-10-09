import { expect, test } from "@playwright/test";
import { mockGcip } from "./fixtures/gcip";

test("organization selection and popup exchange keep GCIP tokens out of browser storage", async ({
  page,
}) => {
  const token = await mockGcip(page);
  await page.route("**/signed-in", (route) =>
    route.fulfill({ contentType: "text/html", body: "<h1>Signed in</h1>" }),
  );
  await page.goto("/sign-in?return_to=%2Fsettings");
  await page.getByLabel("Organization").fill("acme");
  const login = page.waitForRequest("**/auth/login?**");
  await page.getByRole("button", { name: "Continue", exact: true }).click();
  expect(new URL((await login).url()).searchParams.get("return_to")).toBe(
    "/settings",
  );
  await page
    .getByRole("button", { name: "Continue with Google", exact: true })
    .click();
  await expect(page).toHaveURL(/\/signed-in$/);
  const storage = await page.evaluate(() => ({
    local: JSON.stringify(localStorage),
    session: JSON.stringify(sessionStorage),
  }));
  expect(storage.local + storage.session).not.toContain(token);
  expect(storage.local + storage.session).not.toContain("refreshToken");
});

for (const provider of ["oidc.company", "saml.company"])
  test(`popup supports ${provider}`, async ({ page }) => {
    await mockGcip(page);
    await page.route("**/signed-in", (route) =>
      route.fulfill({ body: "Signed in" }),
    );
    await page.goto("/sign-in?state=browser-bound-state");
    await page
      .getByRole("button", { name: `Continue with ${provider}`, exact: true })
      .click();
    await expect(page).toHaveURL(/\/signed-in$/);
  });

test("password signup waits for email verification and respects per-pool settings", async ({
  page,
}) => {
  await mockGcip(page);
  await page.goto("/sign-in?state=browser-bound-state");
  await page.getByLabel("Email", { exact: true }).fill("person@example.test");
  await page.getByLabel("Password", { exact: true }).fill("fixture-password");
  await page
    .getByRole("button", { name: "Create account", exact: true })
    .click();
  await expect(page.getByRole("status")).toHaveText(
    "Check your email to verify your account, then sign in.",
  );
  await page.getByRole("button", { name: "日本語", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "Aidash にサインイン" }),
  ).toBeVisible();
  await page.route("**/auth/gcip/transaction?**", (route) =>
    route.fulfill({
      json: {
        project_id: "fixture-project",
        api_key: "public",
        auth_domain: "fixture-project.firebaseapp.com",
        tenant_id: "pool-a",
        providers: ["password"],
        password_sign_up: false,
      },
    }),
  );
  await page.reload();
  await expect(
    page.getByRole("button", { name: "Create account", exact: true }),
  ).toHaveCount(0);
});

test("self-hosted OIDC never requests the GCIP SDK", async ({ page }) => {
  const modules: string[] = [];
  page.on("request", (request) => modules.push(request.url()));
  await page.route("**/auth/config", (route) =>
    route.fulfill({ json: { enabled: true, provider: "keycloak" } }),
  );
  await page.goto("/sign-in");
  await expect(page.getByRole("status")).toContainText("OIDC");
  expect(modules.some((url) => /gcip-sdk|firebase/.test(url))).toBe(false);
});
