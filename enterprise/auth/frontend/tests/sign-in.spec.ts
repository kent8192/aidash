import { expect, test, type Page } from "@playwright/test";

const flow = "5b3b9f0c-252a-4ca9-8875-3fd88cf40a11";
const context = {
  csrf: "flow-csrf",
  tenant_ids: ["personal"],
  selected_tenant_id: "personal",
  client_id: "marketplace",
  phase: "sign-in",
  relink_only: false,
  providers: ["password", "google.com"],
  password_sign_up: true,
  mfa_mandatory: false,
  firebase: {
    project_id: "browser-fixture",
    api_key: "public-fixture-key",
    auth_domain: "fixture.firebaseapp.com",
  },
};

// Only the external GCIP adapter is replaced. Real UI, flow/CSRF request bodies,
// navigation and two separate human proof steps run in the browser.
async function sdk(page: Page, secondFactor?: "challenge" | "enroll") {
  await page.route("**/src/gcip-sdk.ts", (route) =>
    route.fulfill({
      contentType: "application/javascript",
      body: `export function createClient() { return {
      password: async email => {
        if ('${secondFactor ?? ""}' === 'challenge') throw Object.assign(new Error('second factor due'), { code: 'aidash/second-factor-required' });
        if ('${secondFactor ?? ""}' === 'enroll') throw Object.assign(new Error('enrollment due'), { code: 'aidash/enrollment-required' });
        return 'proof:' + email;
      },
      popup: async provider => 'proof:' + provider,
      redirectResult: async () => null,
      register: async () => true,
      resend: async () => true,
      resolveSecondFactor: async code => 'totp:' + code,
      startEnrollment: async () => ({ secret: 'FIXTURESETUPKEY', uri: 'otpauth://totp/Aidash:test?secret=FIXTURESETUPKEY' }),
      finishEnrollment: async code => 'enroll:' + code,
      close: async () => {}
    }; }`,
    }),
  );
}

test("a new common Account requires an explicit choice after successful GCIP proof", async ({
  page,
}) => {
  await sdk(page);
  let phase = "sign-in";
  await page.route("**/login/context?*", (route) =>
    route.fulfill({ json: { ...context, phase } }),
  );
  await page.route("**/login/verify", async (route) => {
    expect(route.request().postDataJSON()).toEqual({
      flow,
      csrf: "flow-csrf",
      id_token: "proof:person@example.org",
    });
    phase = "choose-account";
    await route.fulfill({ json: { phase } });
  });
  await page.route("**/login/choice", async (route) => {
    expect(route.request().postDataJSON()).toEqual({
      flow,
      csrf: "flow-csrf",
      choice: "new",
    });
    await route.fulfill({ json: { redirect_to: "/signed-in" } });
  });
  await page.route("**/signed-in", (route) =>
    route.fulfill({
      contentType: "text/html",
      body: "<h1>Marketplace callback reached</h1>",
    }),
  );
  await page.goto(`/sign-in?flow=${flow}`);
  await page.getByLabel("Email", { exact: true }).fill("person@example.org");
  await page.getByLabel("Password", { exact: true }).fill("fixture-password");
  await page.getByRole("button", { name: "Sign in with email" }).click();
  await expect(
    page.getByRole("heading", { name: "Choose your Aidash Account" }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "Create a new Aidash Account" })
    .click();
  await expect(
    page.getByRole("heading", { name: "Marketplace callback reached" }),
  ).toBeVisible();
});

test("attaching to an existing Account requires a separate fresh proof and clears prior fields", async ({
  page,
}) => {
  await sdk(page);
  let phase = "sign-in";
  let proofs = 0;
  await page.route("**/login/context?*", (route) =>
    route.fulfill({ json: { ...context, phase } }),
  );
  await page.route("**/login/choice", async (route) => {
    expect(route.request().postDataJSON()).toEqual({
      flow,
      csrf: "flow-csrf",
      choice: "existing",
    });
    phase = "prove-existing";
    await route.fulfill({ json: { phase } });
  });
  await page.route("**/login/verify", async (route) => {
    proofs += 1;
    expect(route.request().postDataJSON()).toEqual({
      flow,
      csrf: "flow-csrf",
      id_token:
        proofs === 1
          ? "proof:new-login@example.org"
          : "proof:original@example.org",
    });
    if (proofs === 1) {
      phase = "choose-account";
      await route.fulfill({ json: { phase } });
      return;
    }
    await route.fulfill({ json: { redirect_to: "/signed-in" } });
  });
  await page.route("**/signed-in", (route) =>
    route.fulfill({
      contentType: "text/html",
      body: "<h1>Linked callback reached</h1>",
    }),
  );
  await page.goto(`/sign-in?flow=${flow}`);
  await page.getByLabel("Email", { exact: true }).fill("new-login@example.org");
  await page
    .getByLabel("Password", { exact: true })
    .fill("first-identity-proof");
  await page.getByRole("button", { name: "Sign in with email" }).click();
  await page
    .getByRole("button", { name: "Use an existing Aidash Account" })
    .click();
  await expect(
    page.getByRole("heading", { name: "Verify your existing Account" }),
  ).toBeVisible();
  await expect(page.getByLabel("Email", { exact: true })).toHaveValue("");
  await expect(page.getByLabel("Password", { exact: true })).toHaveValue("");
  await page.getByLabel("Email", { exact: true }).fill("original@example.org");
  await page
    .getByLabel("Password", { exact: true })
    .fill("fresh-proof-password");
  await page.getByRole("button", { name: "Sign in with email" }).click();
  await expect(
    page.getByRole("heading", { name: "Linked callback reached" }),
  ).toBeVisible();
});

test("a retained historical association offers reattachment rather than a new Account", async ({
  page,
}) => {
  await page.route("**/login/context?*", (route) =>
    route.fulfill({
      json: { ...context, phase: "choose-account", relink_only: true },
    }),
  );
  await page.goto(`/sign-in?flow=${flow}`);
  await expect(
    page.getByRole("button", { name: "Create a new Aidash Account" }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Use an existing Aidash Account" }),
  ).toBeVisible();
  await expect(page.getByText(/original Aidash Account/)).toBeVisible();
});

test("an unavailable flow cannot show a usable credential form", async ({
  page,
}) => {
  await page.route("**/login/context?*", (route) =>
    route.fulfill({
      status: 503,
      json: { error: "Sign-in is temporarily unavailable" },
    }),
  );
  await page.goto(`/sign-in?flow=${flow}`);
  await expect(page.getByRole("alert")).toContainText(
    "Sign-in is temporarily unavailable",
  );
  await expect(page.getByLabel("Password", { exact: true })).toHaveCount(0);
});

test("Account settings send a cookie-bound CSRF proof before starting fresh linking", async ({
  page,
}) => {
  await page.route("http://127.0.0.1:4191/account", (route) =>
    route.fulfill({
      json: {
        account_id: "account-A",
        source_identity_id: "identity-X",
        csrf: "account-csrf",
        identities: [
          {
            id: "identity-X",
            issuer: "https://issuer.example.org",
            subject: "subject-X",
            linked: true,
          },
        ],
      },
    }),
  );
  await page.route("**/account/link/start", async (route) => {
    expect(route.request().postDataJSON()).toEqual({ csrf: "account-csrf" });
    await route.fulfill({ json: { redirect_to: `/sign-in?flow=${flow}` } });
  });
  await page.route("**/login/context?*", (route) =>
    route.fulfill({ json: { ...context, phase: "prove-current" } }),
  );
  await page.goto("/settings/account");
  await expect(page.getByRole("button", { name: "Unlink" })).toBeDisabled();
  await page.getByRole("button", { name: "Add a sign-in method" }).click();
  await expect(
    page.getByRole("heading", { name: "Verify your current Account" }),
  ).toBeVisible();
});

for (const [step, heading, proof] of [
  ["challenge", "Two-step verification", "totp:123456"],
  ["enroll", "Set up two-step verification", "enroll:123456"],
] as const) {
  test(`GCIP ${step} must finish before a proof is submitted to the Authentication Server`, async ({
    page,
  }) => {
    await sdk(page, step);
    let submitted = false;
    await page.route("**/login/context?*", (route) =>
      route.fulfill({ json: context }),
    );
    await page.route("**/login/verify", async (route) => {
      expect(route.request().postDataJSON()).toEqual({
        flow,
        csrf: "flow-csrf",
        id_token: proof,
      });
      submitted = true;
      await route.fulfill({ json: { redirect_to: "/signed-in" } });
    });
    await page.route("**/signed-in", (route) =>
      route.fulfill({
        contentType: "text/html",
        body: "<h1>Verified callback reached</h1>",
      }),
    );
    await page.goto(`/sign-in?flow=${flow}`);
    await page.getByLabel("Email", { exact: true }).fill("person@example.org");
    await page
      .getByLabel("Password", { exact: true })
      .fill("first-factor-only");
    await page.getByRole("button", { name: "Sign in with email" }).click();
    await expect(
      page.getByRole("heading", { name: heading, exact: true }),
    ).toBeVisible();
    expect(submitted).toBe(false);
    if (step === "enroll")
      await expect(
        page.getByText("FIXTURESETUPKEY", { exact: true }),
      ).toBeVisible();
    await page.getByLabel("Verification code", { exact: true }).fill("123456");
    await page.getByRole("button", { name: "Verify code" }).click();
    await expect(
      page.getByRole("heading", { name: "Verified callback reached" }),
    ).toBeVisible();
    expect(submitted).toBe(true);
  });
}

test("changing directory uses a server-confirmed context and only its configured providers", async ({
  page,
}) => {
  await sdk(page);
  let selected = "personal";
  await page.route("**/login/context?*", (route) =>
    route.fulfill({
      json: {
        ...context,
        tenant_ids: ["personal", "enterprise"],
        selected_tenant_id: selected,
        providers:
          selected === "personal"
            ? ["password", "google.com"]
            : ["oidc.organization"],
      },
    }),
  );
  await page.route("**/login/tenant", async (route) => {
    expect(route.request().postDataJSON()).toEqual({
      flow,
      csrf: "flow-csrf",
      tenant_id: "enterprise",
    });
    selected = "enterprise";
    await route.fulfill({ json: { ...context, selected_tenant_id: selected } });
  });
  await page.goto(`/sign-in?flow=${flow}`);
  await page
    .getByRole("combobox", { name: "Sign-in directory", exact: true })
    .selectOption("enterprise");
  await expect(
    page.getByRole("button", { name: "Continue with oidc.organization" }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Continue with Google" }),
  ).toHaveCount(0);
  await expect(page.getByLabel("Password", { exact: true })).toHaveCount(0);
});

test("an unsupported callback scheme is refused before browser navigation", async ({
  page,
}) => {
  await page.route("**/login/context?*", (route) =>
    route.fulfill({ json: { ...context, phase: "choose-account" } }),
  );
  await page.route("**/login/choice", (route) =>
    route.fulfill({
      json: { redirect_to: "javascript:document.title='unsafe'" },
    }),
  );
  await page.goto(`/sign-in?flow=${flow}`);
  await page
    .getByRole("button", { name: "Create a new Aidash Account" })
    .click();
  await expect(page.getByRole("alert")).toContainText("unsupported redirect");
  await expect(page).toHaveTitle("Aidash Account");
});

test("explicit global sign-out sends the Account CSRF proof and removes the protected identity view", async ({
  page,
}) => {
  await page.route("http://127.0.0.1:4191/account", (route) =>
    route.fulfill({
      json: {
        account_id: "account-A",
        source_identity_id: "identity-X",
        csrf: "account-csrf",
        identities: [
          {
            id: "identity-X",
            issuer: "https://issuer.example.org",
            subject: "subject-X",
            linked: true,
          },
        ],
      },
    }),
  );
  await page.route("**/logout/all", async (route) => {
    expect(route.request().postDataJSON()).toEqual({ csrf: "account-csrf" });
    await route.fulfill({ json: { completed: true } });
  });
  await page.goto("/settings/account");
  await page
    .getByRole("button", { name: "Sign out of all products and devices" })
    .click();
  await expect(
    page.getByRole("heading", { name: "Signed out everywhere" }),
  ).toBeVisible();
  await expect(
    page.getByText("https://issuer.example.org", { exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Add a sign-in method" }),
  ).toHaveCount(0);
});
