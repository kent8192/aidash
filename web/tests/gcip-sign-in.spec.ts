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
  await page
    .getByRole("button", { name: "Use your organization ID instead" })
    .click();
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

test("verification email failure can recover after the signup client closes", async ({
  page,
}) => {
  await mockGcip(page);
  // Exercise the real SDK adapter; only Firebase HTTP responses are fixtures.
  await page.unroute("**/src/gcip-sdk.ts");
  const { signedToken } = await import("./fixtures/gcip");
  let created = 0;
  let signedIn = 0;
  let mailAttempts = 0;
  let mailFails = true;
  let verified = false;
  const headers = {
    "access-control-allow-origin": "*",
    "access-control-allow-methods": "POST, OPTIONS",
    "access-control-allow-headers": "*",
  };
  await page.route(
    "https://identitytoolkit.googleapis.com/**",
    async (route) => {
      if (route.request().method() === "OPTIONS") {
        await route.fulfill({ status: 204, headers });
        return;
      }
      const path = new URL(route.request().url()).pathname;
      const body = route.request().postDataJSON();
      if (
        path.endsWith("accounts:signUp") ||
        path.endsWith("accounts:signInWithPassword")
      ) {
        expect(body.tenantId).toBe("pool-a");
      } else {
        expect(
          JSON.parse(
            Buffer.from(body.idToken.split(".")[1], "base64url").toString(),
          ).firebase.tenant,
        ).toBe("pool-a");
      }
      const credential = {
        localId: "person",
        email: "person@example.test",
        idToken: signedToken(verified, "password"),
        refreshToken: "fixture-refresh-token",
        expiresIn: "3600",
      };
      if (path.endsWith("accounts:signUp")) {
        created++;
        await route.fulfill({ headers, json: credential });
      } else if (path.endsWith("accounts:signInWithPassword")) {
        signedIn++;
        await route.fulfill({ headers, json: credential });
      } else if (path.endsWith("accounts:lookup")) {
        await route.fulfill({
          headers,
          json: {
            users: [
              {
                localId: "person",
                email: "person@example.test",
                emailVerified: verified,
                tenantId: "pool-a",
                passwordHash: "fixture",
                providerUserInfo: [
                  {
                    providerId: "password",
                    rawId: "person@example.test",
                    email: "person@example.test",
                  },
                ],
              },
            ],
          },
        });
      } else if (path.endsWith("accounts:sendOobCode")) {
        expect(body.requestType).toBe("VERIFY_EMAIL");
        mailAttempts++;
        await route.fulfill(
          mailFails
            ? {
                headers,
                status: 500,
                json: { error: { message: "INTERNAL_ERROR" } },
              }
            : { headers, json: { email: "person@example.test" } },
        );
      } else {
        throw new Error(`Unexpected Firebase request: ${path}`);
      }
    },
  );
  await page.route("**/signed-in", (route) =>
    route.fulfill({ body: "Signed in" }),
  );
  let exchanges = 0;
  page.on("request", (request) => {
    if (new URL(request.url()).pathname === "/auth/gcip/exchange") exchanges++;
  });
  await page.goto("/sign-in?state=browser-bound-state");
  await page.getByLabel("Email", { exact: true }).fill("person@example.test");
  await page.getByLabel("Password", { exact: true }).fill("fixture-password");
  await page
    .getByRole("button", { name: "Create account", exact: true })
    .click();
  await expect(page.getByRole("status")).toContainText(
    "Your account was created",
  );
  await expect(page.getByLabel("Password", { exact: true })).toHaveValue("");
  // Recovery works after reload, without retaining the authenticated signup client.
  await page.reload();
  await page.getByLabel("Email", { exact: true }).fill("person@example.test");
  await page.getByLabel("Password", { exact: true }).fill("fixture-password");
  await page
    .getByRole("button", { name: "Resend verification email", exact: true })
    .click();
  await expect(page.getByRole("status")).toContainText(
    "Unable to send the verification email",
  );
  await expect(page.getByLabel("Password", { exact: true })).toHaveValue("");
  mailFails = false;
  await page.getByLabel("Password", { exact: true }).fill("fixture-password");
  await page
    .getByRole("button", { name: "Resend verification email", exact: true })
    .click();
  await expect(page.getByRole("status")).toHaveText(
    "Check your email to verify your account, then sign in.",
  );
  expect(created).toBe(1);
  expect(signedIn).toBe(2);
  expect(mailAttempts).toBe(3);
  expect(exchanges).toBe(0);
  const storage = await page.evaluate(
    () => JSON.stringify(localStorage) + JSON.stringify(sessionStorage),
  );
  expect(storage).not.toContain("fixture-refresh-token");
  expect(storage).not.toContain("idToken");
  verified = true;
  await page.getByLabel("Password", { exact: true }).fill("fixture-password");
  await page
    .getByRole("button", { name: "Resend verification email", exact: true })
    .click();
  await expect(page.getByRole("status")).toHaveText(
    "Your email is already verified. Sign in to continue.",
  );
  expect(mailAttempts).toBe(3);
  expect(exchanges).toBe(0);
  await page.getByLabel("Password", { exact: true }).fill("fixture-password");
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(/\/signed-in$/);
  expect(exchanges).toBe(1);
});

test("email discovery posts the form and hands the email to the state page once", async ({
  page,
}) => {
  await mockGcip(page);
  await page.goto("/sign-in?return_to=%2Fsettings");
  await page.getByLabel("Email", { exact: true }).fill("  person@acme.test ");
  const login = page.waitForRequest(
    (request) =>
      new URL(request.url()).pathname === "/auth/login" &&
      request.method() === "POST",
  );
  // Park the redirect on a static page to observe the handoff before the
  // state page consumes it.
  await page.route("**/auth/login", (route) =>
    route.fulfill({ status: 303, headers: { location: "/held" }, body: "" }),
  );
  await page.route("**/held", (route) =>
    route.fulfill({ contentType: "text/html", body: "<p>held</p>" }),
  );
  await page.getByRole("button", { name: "Continue", exact: true }).click();
  const form = new URLSearchParams((await login).postData() ?? "");
  expect(form.get("email")).toBe("person@acme.test");
  expect(form.get("return_to")).toBe("/settings");
  await expect(page).toHaveURL(/\/held$/);
  expect(
    await page.evaluate(() => sessionStorage.getItem("aidash.gcip.email")),
  ).toBe("person@acme.test");
  await page.goto("/sign-in?state=browser-bound-state");
  await expect(page.getByLabel("Email", { exact: true })).toHaveValue(
    "person@acme.test",
  );
  expect(
    await page.evaluate(() => sessionStorage.getItem("aidash.gcip.email")),
  ).toBeNull();
  await page.route("**/signed-in", (route) =>
    route.fulfill({ body: "Signed in" }),
  );
  await page
    .getByRole("button", { name: "Continue with oidc.company", exact: true })
    .click();
  await expect(page).toHaveURL(/\/signed-in$/);
});

test("state page prefills the email and passes it as a popup login hint", async ({
  page,
}) => {
  await mockGcip(page);
  await page.goto("/sign-in");
  await page.evaluate(() =>
    sessionStorage.setItem("aidash.gcip.email", "person@acme.test"),
  );
  await page.goto("/sign-in?state=browser-bound-state");
  await expect(page.getByLabel("Email", { exact: true })).toHaveValue(
    "person@acme.test",
  );
  expect(
    await page.evaluate(() => sessionStorage.getItem("aidash.gcip.email")),
  ).toBeNull();
  // Stop after the popup so the page stays put for inspection.
  await page.route("**/auth/gcip/exchange", (route) =>
    route.fulfill({ status: 500, body: "" }),
  );
  await page
    .getByRole("button", { name: "Continue with Google", exact: true })
    .click();
  await expect(page.getByRole("status")).toHaveText(
    "Sign-in failed. Please try again.",
  );
  const calls = await page.evaluate(
    () => (window as unknown as { gcipCalls: { kind: string }[] }).gcipCalls,
  );
  expect(calls.find((call) => call.kind === "popup")).toEqual({
    kind: "popup",
    id: "google.com",
    hint: "person@acme.test",
  });
});

test("unknown sign-in entries show one generic message", async ({ page }) => {
  await mockGcip(page);
  await page.goto("/sign-in?return_to=%2Fsettings");
  await page.getByLabel("Email", { exact: true }).fill("person@unknown.test");
  await page.getByRole("button", { name: "Continue", exact: true }).click();
  await expect(page).toHaveURL(
    /\/sign-in\?error=not_found&return_to=%2Fsettings$/,
  );
  await expect(page.getByRole("alert")).toHaveText(
    "We couldn't find a sign-in for that entry. Check it, or contact your administrator.",
  );
  // The email form still carries return_to for the retry.
  await expect(page.locator('input[name="return_to"]')).toHaveValue(
    "/settings",
  );
  await page.getByRole("button", { name: "日本語", exact: true }).click();
  await expect(page.getByRole("alert")).toHaveText(
    "サインイン先が見つかりません。入力内容を確認するか、管理者に連絡してください。",
  );
});

test("organization link reveals the org form and clears the email handoff", async ({
  page,
}) => {
  await mockGcip(page);
  await page.goto("/sign-in?return_to=%2Fsettings");
  await page.evaluate(() =>
    sessionStorage.setItem("aidash.gcip.email", "stale@acme.test"),
  );
  await expect(page.getByLabel("Organization")).toHaveCount(0);
  await page
    .getByRole("button", { name: "Use your organization ID instead" })
    .click();
  await page.getByLabel("Organization").fill("acme");
  const login = page.waitForRequest("**/auth/login?**");
  await page.getByRole("button", { name: "Continue", exact: true }).click();
  const url = new URL((await login).url());
  expect(url.searchParams.get("org")).toBe("acme");
  expect(url.searchParams.get("return_to")).toBe("/settings");
  await expect(page).toHaveURL(/state=browser-bound-state$/);
  await expect(page.getByLabel("Email", { exact: true })).toHaveValue("");
  expect(
    await page.evaluate(() => sessionStorage.getItem("aidash.gcip.email")),
  ).toBeNull();
});
