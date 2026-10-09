import { readFileSync } from "node:fs";
import { createSign, createVerify } from "node:crypto";
import { expect, type Page } from "@playwright/test";
const privateKey = readFileSync(
  new URL(
    "../../../server/src/apps/execution/tests/fixtures/oidc/signing-test-only.pem",
    import.meta.url,
  ),
);
export function signedToken(emailVerified = true, provider = "google.com") {
  const now = Math.floor(Date.now() / 1000);
  const header = Buffer.from(
    JSON.stringify({ alg: "RS256", kid: "fixture" }),
  ).toString("base64url");
  const payload = Buffer.from(
    JSON.stringify({
      iss: "https://securetoken.google.com/fixture-project",
      aud: "fixture-project",
      sub: "person",
      iat: now,
      exp: now + 3600,
      auth_time: now,
      firebase: { tenant: "pool-a", sign_in_provider: provider },
      email: "person@example.test",
      email_verified: emailVerified,
    }),
  ).toString("base64url");
  const content = `${header}.${payload}`;
  return `${content}.${createSign("RSA-SHA256").update(content).sign(privateKey).toString("base64url")}`;
}
export async function mockGcip(
  page: Page,
  destination = "/signed-in",
  signUp = true,
) {
  const token = signedToken();
  await page.addInitScript(() =>
    localStorage.setItem("aidash-locale", "en-US"),
  );
  await page.route("**/auth/config", (route) =>
    route.fulfill({ json: { enabled: true, provider: "gcip" } }),
  );
  await page.route("**/src/gcip-sdk.ts", (route) =>
    route.fulfill({
      contentType: "text/javascript",
      body: `export function createClient(config) { window.gcipCalls ??= []; window.gcipCalls.push({kind:'create',tenant:config.tenant_id}); return { popup: async id => {window.gcipCalls.push({kind:'popup',id});return ${JSON.stringify(token)}}, password: async () => ${JSON.stringify(token)}, register: async () => { window.gcipCalls.push({kind:'register'}); return true; }, resend: async () => { window.gcipCalls.push({kind:'resend'}); return true; }, close: async () => window.gcipCalls.push({kind:'close'}) }; }`,
    }),
  );
  await page.route("**/auth/gcip/transaction?**", (route) =>
    route.fulfill({
      json: {
        project_id: "fixture-project",
        api_key: "public-key",
        auth_domain: "fixture-project.firebaseapp.com",
        tenant_id: "pool-a",
        providers: ["google.com", "oidc.company", "saml.company", "password"],
        password_sign_up: signUp,
      },
    }),
  );
  await page.route("**/auth/login?**", (route) => {
    const url = new URL(route.request().url());
    expect(url.searchParams.get("org")).toBe("acme");
    return route.fulfill({
      status: 303,
      headers: { location: "/sign-in?state=browser-bound-state" },
      body: "",
    });
  });
  await page.route("**/auth/gcip/exchange", (route) => {
    const body = route.request().postDataJSON();
    expect(body.state).toBe("browser-bound-state");
    const [header, payload, signature] = body.id_token.split(".");
    expect(
      createVerify("RSA-SHA256")
        .update(`${header}.${payload}`)
        .verify(privateKey, Buffer.from(signature, "base64url")),
    ).toBe(true);
    expect(
      JSON.parse(Buffer.from(payload, "base64url").toString()).firebase.tenant,
    ).toBe("pool-a");
    expect(
      JSON.parse(Buffer.from(payload, "base64url").toString()).email_verified,
    ).toBe(true);
    return route.fulfill({
      json: { return_to: destination },
      headers: { "cache-control": "no-store" },
    });
  });
  return token;
}
