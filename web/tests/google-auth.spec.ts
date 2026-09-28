import { expect, test } from "@playwright/test";

for (const [locale, provider, label] of [
  ["ja-JP", "google", "Google でサインイン"],
  ["en-US", "google", "Sign in with Google"],
  ["en-US", "keycloak", "Sign in with Keycloak"],
]) {
  test(`${provider} login in ${locale} preserves the destination`, async ({
    page,
  }) => {
    await page.addInitScript(
      (locale) => localStorage.setItem("aidash-locale", locale),
      locale,
    );
    await page.route("**/auth/config", (route) =>
      route.fulfill({
        json: { enabled: true, provider, login_url: "/auth/login" },
      }),
    );
    await page.route("**/auth/session", (route) =>
      route.fulfill({ status: 401, body: "" }),
    );
    await page.route("**/auth/login?**", (route) =>
      route.fulfill({
        contentType: "text/plain",
        body: "OAuth redirect fixture",
      }),
    );
    await page.goto("/settings");
    await page.getByRole("button", { name: label, exact: true }).click();
    await expect(page).toHaveURL(/\/auth\/login\?return_to=%2Fsettings$/);
  });
}
