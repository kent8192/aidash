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
