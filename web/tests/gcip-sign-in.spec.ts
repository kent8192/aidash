import { expect, test, type Page } from "@playwright/test";
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

// Same-origin helper state is GCIP state too: none may outlive an attempt.
async function expectNoFirebaseResidue(page: Page, token: string) {
  const residue = await page.evaluate(async () => ({
    storage: JSON.stringify(localStorage) + JSON.stringify(sessionStorage),
    databases: (await indexedDB.databases()).map((database) => database.name),
  }));
  expect(residue.storage).not.toContain(token);
  expect(residue.storage).not.toContain("refreshToken");
  expect(residue.storage).not.toContain("firebase:");
  expect(residue.databases).not.toContain("firebaseLocalStorageDb");
}
const calls = (page: Page) =>
  page.evaluate(
    () =>
      JSON.parse(sessionStorage.getItem("fixture-calls") ?? "[]") as string[],
  );

test("a blocked popup falls back to redirect and resumes the same transaction", async ({
  page,
}) => {
  const token = await mockGcip(page);
  await page.addInitScript(() =>
    sessionStorage.setItem("fixture-popup-error", "auth/popup-blocked"),
  );
  await page.route("**/signed-in", (route) =>
    route.fulfill({ contentType: "text/html", body: "<h1>Signed in</h1>" }),
  );
  const exchanges: string[] = [];
  page.on("request", (request) => {
    if (new URL(request.url()).pathname === "/auth/gcip/exchange")
      exchanges.push(request.postDataJSON().state);
  });
  await page.goto("/sign-in?state=browser-bound-state");
  await page
    .getByRole("button", { name: "Continue with Google", exact: true })
    .click();
  await expect(page).toHaveURL(/\/signed-in$/);
  expect(exchanges).toEqual(["browser-bound-state"]);
  expect(await calls(page)).toEqual([
    "create",
    "redirectResult",
    "close",
    "create",
    "popup:google.com",
    "redirect:google.com",
    "create",
    "redirectResult",
    "close",
  ]);
  await expectNoFirebaseResidue(page, token);
});

test("a popup the user closes does not start a redirect", async ({ page }) => {
  await mockGcip(page);
  await page.addInitScript(() =>
    sessionStorage.setItem("fixture-popup-error", "auth/popup-closed-by-user"),
  );
  await page.goto("/sign-in?state=browser-bound-state");
  await page
    .getByRole("button", { name: "Continue with Google", exact: true })
    .click();
  await expect(page.getByRole("status")).toContainText("Sign-in failed");
  expect(await calls(page)).not.toContain("redirect:google.com");
});

test("a failed redirect exchange keeps the form and clears Firebase state", async ({
  page,
}) => {
  const token = await mockGcip(page);
  await page.route("**/auth/gcip/exchange", (route) =>
    route.fulfill({ status: 401, body: "" }),
  );
  await page.addInitScript(() =>
    sessionStorage.setItem(
      "firebase:pendingRedirect:public-key:aidash-gcip",
      "true",
    ),
  );
  await page.goto("/sign-in?state=browser-bound-state");
  await expect(page.getByRole("status")).toContainText("Sign-in failed");
  await expect(
    page.getByRole("button", { name: "Continue with Google", exact: true }),
  ).toBeEnabled();
  await expectNoFirebaseResidue(page, token);
});

test("an expired transaction on redirect return clears Firebase state", async ({
  page,
}) => {
  const token = await mockGcip(page);
  await page.route("**/auth/gcip/transaction?**", (route) =>
    route.fulfill({ status: 410, body: "" }),
  );
  await page.addInitScript(() => {
    sessionStorage.setItem(
      "firebase:pendingRedirect:public-key:aidash-gcip",
      "true",
    );
    indexedDB.open("firebaseLocalStorageDb").onsuccess = (event) =>
      (event.target as IDBOpenDBRequest).result.close();
  });
  await page.goto("/sign-in?state=browser-bound-state");
  await expect(page.getByRole("status")).toContainText("Sign-in failed");
  expect(await calls(page)).toEqual([]);
  await expect
    .poll(
      async () =>
        await page.evaluate(async () =>
          (await indexedDB.databases()).map((database) => database.name),
        ),
    )
    .not.toContain("firebaseLocalStorageDb");
  await expectNoFirebaseResidue(page, token);
});

test("changing language during a popup keeps the active client", async ({
  page,
}) => {
  await mockGcip(page);
  await page.addInitScript(() =>
    sessionStorage.setItem("fixture-popup-hold", "true"),
  );
  await page.route("**/signed-in", (route) =>
    route.fulfill({ body: "Signed in" }),
  );
  let transactions = 0;
  page.on("request", (request) => {
    if (new URL(request.url()).pathname === "/auth/gcip/transaction")
      transactions++;
  });
  await page.goto("/sign-in?state=browser-bound-state");
  await page
    .getByRole("button", { name: "Continue with Google", exact: true })
    .click();
  await expect.poll(() => calls(page)).toContain("popup:google.com");
  const loaded = transactions;
  await page.getByRole("button", { name: "日本語", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "Aidash にサインイン" }),
  ).toBeVisible();
  // No redirect recovery reran, so nothing released the in-flight client.
  expect(transactions).toBe(loaded);
  expect(await calls(page)).toEqual([
    "create",
    "redirectResult",
    "close",
    "create",
    "popup:google.com",
  ]);
  await page.evaluate(() =>
    (
      window as unknown as { fixtureFinishPopup: () => void }
    ).fixtureFinishPopup(),
  );
  await expect(page).toHaveURL(/\/signed-in$/);
});

test("a cleanup failure after a successful exchange still navigates", async ({
  page,
}) => {
  await mockGcip(page);
  await page.addInitScript(() =>
    sessionStorage.setItem("fixture-close-error", "true"),
  );
  await page.route("**/signed-in", (route) =>
    route.fulfill({ body: "Signed in" }),
  );
  await page.goto("/sign-in?state=browser-bound-state");
  await page
    .getByRole("button", { name: "Continue with Google", exact: true })
    .click();
  await expect(page).toHaveURL(/\/signed-in$/);
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
    route.fulfill({
      status: 303,
      headers: { location: "/held?state=browser-bound-state" },
      body: "",
    }),
  );
  await page.route("**/held?**", (route) =>
    route.fulfill({ contentType: "text/html", body: "<p>held</p>" }),
  );
  await page.getByRole("button", { name: "Continue", exact: true }).click();
  const form = new URLSearchParams((await login).postData() ?? "");
  expect(form.get("email")).toBe("person@acme.test");
  expect(form.get("return_to")).toBe("/settings");
  await expect(page).toHaveURL(/\/held\?state=browser-bound-state$/);
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

test("discovery errors answered directly never store the email handoff", async ({
  page,
}) => {
  await mockGcip(page);
  await page.goto("/sign-in?return_to=%2Fsettings");
  // e.g. the gateway's discovery budget is exhausted.
  await page.route("**/auth/login", (route) =>
    route.fulfill({ status: 429, body: "rate limit exceeded" }),
  );
  await page.getByLabel("Email", { exact: true }).fill("person@acme.test");
  await page.getByRole("button", { name: "Continue", exact: true }).click();
  await expect(page.getByRole("status")).toHaveText(
    "Sign-in failed. Please try again.",
  );
  await expect(page).toHaveURL(/\/sign-in\?return_to=%2Fsettings$/);
  expect(
    await page.evaluate(() => sessionStorage.getItem("aidash.gcip.email")),
  ).toBeNull();
  // The form recovers for a retry.
  await expect(
    page.getByRole("button", { name: "Continue", exact: true }),
  ).toBeEnabled();
});

test("state page prefills the email and passes it as a popup login hint", async ({
  page,
}) => {
  await mockGcip(page);
  await page.goto("/sign-in");
  // The stateless page clears stale handoffs on mount; seed after it renders.
  await expect(page.getByLabel("Email", { exact: true })).toBeVisible();
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
  expect(await calls(page)).toContain("popup:google.com");
  expect(
    await page.evaluate(() => sessionStorage.getItem("fixture-login-hint")),
  ).toBe("person@acme.test");
});

test("a failed auth config load still consumes the email handoff", async ({
  page,
}) => {
  await mockGcip(page);
  await page.goto("/sign-in");
  await expect(page.getByLabel("Email", { exact: true })).toBeVisible();
  await page.evaluate(() =>
    sessionStorage.setItem("aidash.gcip.email", "person@acme.test"),
  );
  const config = page.waitForResponse("**/auth/config");
  await page.route("**/auth/config", (route) =>
    route.fulfill({ status: 503, body: "" }),
  );
  await page.goto("/sign-in?state=browser-bound-state");
  await config;
  expect(
    await page.evaluate(() => sessionStorage.getItem("aidash.gcip.email")),
  ).toBeNull();
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
  // The rejected address does not outlive the failed discovery.
  expect(
    await page.evaluate(() => sessionStorage.getItem("aidash.gcip.email")),
  ).toBeNull();
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

// A second factor is owed after the first one succeeds; the fixture SDK mimics
// the real adapter's rejection codes, and codes pick the TOTP outcome:
// 000000 wrong, 111111 expired, 222222 too many attempts, 333333 unexpected.
async function openSecondFactor(
  page: Page,
  kind: "challenge" | "enroll",
  mfaMandatory = false,
) {
  const token = await mockGcip(page, "/signed-in", true, mfaMandatory);
  await page.addInitScript(
    (kind) => sessionStorage.setItem("fixture-second-factor", kind),
    kind,
  );
  await page.route("**/signed-in", (route) =>
    route.fulfill({ contentType: "text/html", body: "<h1>Signed in</h1>" }),
  );
  await page.goto("/sign-in?state=browser-bound-state");
  return token;
}
const exchangesOf = (page: Page) => {
  const states: string[] = [];
  page.on("request", (request) => {
    if (new URL(request.url()).pathname === "/auth/gcip/exchange")
      states.push(request.postDataJSON().state);
  });
  return states;
};

test("a TOTP challenge after a popup sign-in is answered before the exchange", async ({
  page,
}) => {
  const exchanges = exchangesOf(page);
  await openSecondFactor(page, "challenge");
  await page
    .getByRole("button", { name: "Continue with Google", exact: true })
    .click();
  await expect(
    page.getByRole("heading", { name: "Two-step verification" }),
  ).toBeVisible();
  // The first factor alone must never reach the server.
  expect(exchanges).toEqual([]);
  const code = page.getByLabel("Verification code");
  await expect(code).toHaveAttribute("autocomplete", "one-time-code");
  await expect(code).toHaveAttribute("inputmode", "numeric");
  await expect(
    page.getByRole("button", { name: "Verify", exact: true }),
  ).toBeDisabled();
  await code.fill("000000");
  await page.getByRole("button", { name: "Verify", exact: true }).click();
  await expect(page.getByRole("alert")).toHaveText(
    "That code is not correct. Check the app and try again.",
  );
  await expect(code).toHaveAttribute("aria-invalid", "true");
  await expect(code).toHaveAttribute("aria-describedby", "totp-error");
  await expect(code).toHaveValue("");
  expect(exchanges).toEqual([]);
  await code.fill("123456");
  await page.getByRole("button", { name: "Verify", exact: true }).click();
  await expect(page).toHaveURL(/\/signed-in$/);
  expect(exchanges).toEqual(["browser-bound-state"]);
  expect(await calls(page)).toEqual([
    "create",
    "redirectResult",
    "close",
    "create",
    "popup:google.com",
    "resolve:000000",
    "resolve:123456",
    "close",
  ]);
});

test("a TOTP challenge after a password sign-in uses the same verification", async ({
  page,
}) => {
  const exchanges = exchangesOf(page);
  await openSecondFactor(page, "challenge");
  await page.getByLabel("Email", { exact: true }).fill("person@example.test");
  await page.getByLabel("Password", { exact: true }).fill("fixture-password");
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await page.getByLabel("Verification code").fill("123456");
  await page.getByRole("button", { name: "Verify", exact: true }).click();
  await expect(page).toHaveURL(/\/signed-in$/);
  expect(exchanges).toEqual(["browser-bound-state"]);
});

test("an expired challenge restarts the sign-in and a throttled one can wait", async ({
  page,
}) => {
  const token = await openSecondFactor(page, "challenge");
  await page
    .getByRole("button", { name: "Continue with Google", exact: true })
    .click();
  const code = page.getByLabel("Verification code");
  await code.fill("222222");
  await page.getByRole("button", { name: "Verify", exact: true }).click();
  await expect(page.getByRole("alert")).toHaveText(
    "Too many attempts. Wait a few minutes, then enter a new code.",
  );
  // Throttling leaves the pending sign-in in place for a later attempt.
  await expect(code).toBeVisible();
  await code.fill("111111");
  await page.getByRole("button", { name: "Verify", exact: true }).click();
  await expect(page.getByRole("alert")).toHaveText(
    "This verification expired. Start over to sign in again.",
  );
  await expect(code).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Continue with Google", exact: true }),
  ).toBeVisible();
  // The pending client was closed and no GCIP state is left behind.
  expect((await calls(page)).slice(-1)).toEqual(["close"]);
  await expectNoFirebaseResidue(page, token);
});

test("cancelling a challenge closes the pending sign-in without an exchange", async ({
  page,
}) => {
  const exchanges = exchangesOf(page);
  await openSecondFactor(page, "challenge");
  await page
    .getByRole("button", { name: "Continue with Google", exact: true })
    .click();
  await page.getByRole("button", { name: "Cancel", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "Continue with Google", exact: true }),
  ).toBeVisible();
  expect((await calls(page)).slice(-1)).toEqual(["close"]);
  expect(exchanges).toEqual([]);
});

test("an unexpected verification failure reports a generic error and keeps the form", async ({
  page,
}) => {
  await openSecondFactor(page, "challenge");
  await page
    .getByRole("button", { name: "Continue with Google", exact: true })
    .click();
  await page.getByLabel("Verification code").fill("333333");
  await page.getByRole("button", { name: "Verify", exact: true }).click();
  await expect(page.getByRole("alert")).toHaveText(
    "Verification failed. Please try again.",
  );
  await expect(page.getByLabel("Verification code")).toBeVisible();
});

test("a challenge that surfaces after the redirect fallback is answered on return", async ({
  page,
}) => {
  const exchanges = exchangesOf(page);
  await mockGcip(page);
  await page.addInitScript(() => {
    sessionStorage.setItem("fixture-popup-error", "auth/popup-blocked");
    sessionStorage.setItem("fixture-second-factor", "challenge");
  });
  await page.route("**/signed-in", (route) =>
    route.fulfill({ contentType: "text/html", body: "<h1>Signed in</h1>" }),
  );
  await page.goto("/sign-in?state=browser-bound-state");
  await page
    .getByRole("button", { name: "Continue with Google", exact: true })
    .click();
  // The page reloads from the provider round trip with the challenge pending.
  await expect(
    page.getByRole("heading", { name: "Two-step verification" }),
  ).toBeVisible();
  expect(exchanges).toEqual([]);
  await page.getByLabel("Verification code").fill("123456");
  await page.getByRole("button", { name: "Verify", exact: true }).click();
  await expect(page).toHaveURL(/\/signed-in$/);
  expect(exchanges).toEqual(["browser-bound-state"]);
  expect((await calls(page)).slice(-3)).toEqual([
    "redirectResult",
    "resolve:123456",
    "close",
  ]);
});

test("a mandatory tenant leads a user without a factor through enrollment before the exchange", async ({
  page,
}) => {
  const exchanges = exchangesOf(page);
  await openSecondFactor(page, "enroll", true);
  await page
    .getByRole("button", { name: "Continue with Google", exact: true })
    .click();
  await expect(
    page.getByRole("heading", { name: "Set up two-step verification" }),
  ).toBeVisible();
  await expect(page.getByText("Your organization requires")).toBeVisible();
  await expect(page.getByTestId("totp-secret")).toHaveText("JBSWY3DPEHPK3PXP");
  await expect(
    page.getByRole("link", { name: "Open in authenticator app" }),
  ).toHaveAttribute("href", /^otpauth:\/\/totp\//);
  expect(exchanges).toEqual([]);
  await page.getByLabel("Verification code").fill("000000");
  await page.getByRole("button", { name: "Verify", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("not correct");
  expect(exchanges).toEqual([]);
  await page.getByLabel("Verification code").fill("123456");
  await page.getByRole("button", { name: "Verify", exact: true }).click();
  await expect(page).toHaveURL(/\/signed-in$/);
  expect(exchanges).toEqual(["browser-bound-state"]);
  expect((await calls(page)).slice(-5)).toEqual([
    "popup:google.com",
    "startEnrollment",
    "enroll:000000",
    "enroll:123456",
    "close",
  ]);
});

test("an optional enrollment started from settings does not claim the tenant requires it", async ({
  page,
}) => {
  await mockGcip(page);
  // Init scripts rerun on every navigation; seed the intent once.
  await page.addInitScript(() => {
    if (sessionStorage.getItem("fixture-seeded")) return;
    sessionStorage.setItem("fixture-seeded", "true");
    sessionStorage.setItem("aidash.gcip.enroll", String(Date.now()));
  });
  await page.route("**/signed-in", (route) =>
    route.fulfill({ contentType: "text/html", body: "<h1>Signed in</h1>" }),
  );
  await page.goto("/sign-in?state=browser-bound-state");
  await page
    .getByRole("button", { name: "Continue with Google", exact: true })
    .click();
  await expect(
    page.getByRole("heading", { name: "Set up two-step verification" }),
  ).toBeVisible();
  await expect(page.getByText("Your organization requires")).toHaveCount(0);
  expect(await calls(page)).toContain("intent:enroll");
  await page.getByLabel("Verification code").fill("123456");
  await page.getByRole("button", { name: "Verify", exact: true }).click();
  await expect(page).toHaveURL(/\/signed-in$/);
  // The intent is spent by the completed sign-in.
  expect(
    await page.evaluate(() => sessionStorage.getItem("aidash.gcip.enroll")),
  ).toBeNull();
});

test("an enrollment intent without a transaction is stale and dropped", async ({
  page,
}) => {
  await mockGcip(page);
  await page.addInitScript(() =>
    sessionStorage.setItem("aidash.gcip.enroll", String(Date.now())),
  );
  await page.goto("/sign-in");
  await expect(page.getByLabel("Email", { exact: true })).toBeVisible();
  expect(
    await page.evaluate(() => sessionStorage.getItem("aidash.gcip.enroll")),
  ).toBeNull();
});

test("second-factor copy follows the selected language", async ({ page }) => {
  await openSecondFactor(page, "challenge");
  await page.getByRole("button", { name: "日本語", exact: true }).click();
  await page
    .getByRole("button", { name: "続ける Google", exact: true })
    .click();
  await expect(page.getByRole("heading", { name: "2 段階認証" })).toBeVisible();
  await page.getByLabel("確認コード").fill("000000");
  await page.getByRole("button", { name: "確認", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText(
    "コードが正しくありません",
  );
});
