import type { Page } from "@playwright/test";

export async function selectDashboardLanguage(
  page: Page,
  locale: "ja-JP" | "en-US",
) {
  const accountMenu = page.locator(".account-popover");
  await accountMenu.locator("summary").click();
  await page.getByTestId("language-selector").selectOption(locale);
  await accountMenu.locator("summary").click();
}

/** Keep legacy Bearer API fixtures while exercising the OIDC dashboard UI. */
export async function installBearerDashboard(
  page: Page,
  token: string,
  subject?: { tenant: string; name: string },
) {
  let currentToken = token;
  const sessionId = "fixture-browser-session";
  const context = subject ? "mapping:fixture-mapping" : "operator";
  await page.addInitScript(
    ({ sessionId, context }) => {
      sessionStorage.setItem("aidash-session-id", sessionId);
      sessionStorage.setItem("aidash-context", context);
    },
    { sessionId, context },
  );
  await page.route("**/auth/config", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ enabled: true, login_url: "/auth/login" }),
    }),
  );
  await page.route("**/auth/session", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        id: sessionId,
        operator: !subject,
        mappings: subject
          ? [
              {
                id: "fixture-mapping",
                tenant: subject.tenant,
                subject: subject.name,
              },
            ]
          : [],
      }),
    }),
  );
  await page.route("**/auth/registration", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: "null",
    }),
  );
  await page.route("**/auth/activity", (route) =>
    route.fulfill({ status: 204 }),
  );
  await page.route("**/auth/logout**", (route) =>
    route.fulfill({ status: 204 }),
  );
  await page.route("**/api/**", (route) =>
    route.continue({
      headers: {
        ...route.request().headers(),
        authorization: `Bearer ${currentToken}`,
      },
    }),
  );
  return {
    setToken(value: string) {
      currentToken = value;
    },
  };
}
