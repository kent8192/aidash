import type { Page } from "@playwright/test";

export async function selectDashboardLanguage(
  page: Page,
  locale: "ja-JP" | "en-US",
) {
  await page
    .getByRole("button", { name: /^(Account settings|アカウント設定)$/ })
    .click();
  await page.getByTestId("language-selector").selectOption(locale);
  await page.keyboard.press("Escape");
}

/** The Tenant subject golden_path.py runs its research cluster as. Only a
 *  Tenant subject creates workspaces and runs work (#371). */
export function acceptanceSubject() {
  const token = process.env.AIDASH_E2E_TOKEN;
  const tenant = process.env.AIDASH_E2E_TENANT;
  const name = process.env.AIDASH_E2E_SUBJECT;
  if (!token || !tenant || !name)
    throw new Error(
      "AIDASH_E2E_TOKEN, AIDASH_E2E_TENANT and AIDASH_E2E_SUBJECT are required",
    );
  return { token, tenant, name };
}

/** Keep legacy Bearer API fixtures while exercising the OIDC dashboard UI. */
export async function installBearerDashboard(
  page: Page,
  token: string,
  subject?: { tenant: string; name: string },
  selectedContext?: string | null,
) {
  let currentToken = token;
  const sessionId = "fixture-browser-session";
  const context =
    selectedContext !== undefined
      ? selectedContext
      : subject
        ? `mapping:${subject.tenant}:fixture-mapping`
        : "deployment_administrator";
  await page.addInitScript(
    ({ sessionId, context }) => {
      sessionStorage.setItem("aidash-session-id", sessionId);
      if (context) sessionStorage.setItem("aidash-context", context);
      else sessionStorage.removeItem("aidash-context");
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
        deployment_administrator: !subject,
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
