// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

import { readFileSync } from "node:fs";
import { createSign, createVerify } from "node:crypto";
import { expect, type Page } from "@playwright/test";
const privateKey = readFileSync(
  new URL(
    "../../../../server/src/apps/execution/tests/fixtures/oidc/signing-test-only.pem",
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
  mfaMandatory = false,
) {
  const token = signedToken();
  await page.addInitScript(() =>
    localStorage.setItem("aidash-locale", "en-US"),
  );
  await page.route("**/auth/config", (route) =>
    route.fulfill({ json: { enabled: true, provider: "gcip" } }),
  );
  // The stub mimics what Firebase leaves on this origin once the helper is
  // same-origin: a pending-redirect flag, a cached user and its IndexedDB store.
  const pending = "firebase:pendingRedirect:public-key:aidash-gcip";
  await page.route("**/src/gcip-sdk.ts", (route) =>
    route.fulfill({
      contentType: "text/javascript",
      body: `
const token = ${JSON.stringify(token)};
// Calls survive the redirect round trip; clearing removes only firebase: keys.
const record = (call) => sessionStorage.setItem("fixture-calls", JSON.stringify([...JSON.parse(sessionStorage.getItem("fixture-calls") ?? "[]"), call.id ? call.kind + ":" + call.id : call.kind]));
const secondFactor = (options) => sessionStorage.getItem("fixture-second-factor") || (options.enroll ? "enroll" : "");
// Mimics the SDK: a first factor that leaves a second factor owed rejects with its code.
const owed = (options) => {
  const kind = secondFactor(options);
  if (kind) throw Object.assign(new Error(kind), { code: kind === "challenge" ? "aidash/second-factor-required" : "aidash/enrollment-required" });
};
// Codes pick the outcome: the SDK errors a TOTP code can meet.
const verdict = (code) => {
  const failure = { "000000": "auth/invalid-verification-code", "111111": "auth/code-expired", "222222": "auth/too-many-requests", "333333": "auth/internal-error" }[code];
  if (failure) throw Object.assign(new Error(failure), { code: failure });
};
export function createClient(config, options = {}) {
  record({ kind: "create" });
  if (options.enroll) record({ kind: "intent", id: "enroll" });
  return {
    popup: async (id, hint) => {
      record({ kind: "popup", id });
      if (hint) sessionStorage.setItem("fixture-login-hint", hint);
      // Lets a test act while the provider popup is still open.
      if (sessionStorage.getItem("fixture-popup-hold"))
        await new Promise((resolve) => { window.fixtureFinishPopup = resolve; });
      const code = sessionStorage.getItem("fixture-popup-error");
      if (code) throw Object.assign(new Error(code), { code });
      owed(options);
      return token;
    },
    redirect: (id) => {
      record({ kind: "redirect", id });
      sessionStorage.setItem(${JSON.stringify(pending)}, "true");
      // The provider round trip returns to the page that started it.
      window.location.assign(window.location.href);
      return new Promise(() => {});
    },
    redirectResult: async () => {
      record({ kind: "redirectResult" });
      if (sessionStorage.getItem(${JSON.stringify(pending)}) !== "true") return null;
      localStorage.setItem("firebase:authUser:public-key:aidash-gcip", JSON.stringify({ stsTokenManager: { accessToken: token, refreshToken: "fixture-refresh-token" } }));
      await new Promise((resolve) => { const request = indexedDB.open("firebaseLocalStorageDb"); request.onsuccess = () => { request.result.close(); resolve(); }; });
      owed(options);
      return token;
    },
    password: async () => { owed(options); return token; },
    // Like the adapter: an enrollment intent continues past a resolved challenge.
    resolveSecondFactor: async (code) => { record({ kind: "resolve", id: code }); verdict(code); if (options.enroll) throw Object.assign(new Error("enroll"), { code: "aidash/enrollment-required" }); return token; },
    startEnrollment: async () => {
      record({ kind: "startEnrollment" });
      return { secret: "JBSWY3DPEHPK3PXP", uri: "otpauth://totp/Aidash:person@example.test?secret=JBSWY3DPEHPK3PXP&issuer=Aidash" };
    },
    finishEnrollment: async (code) => { record({ kind: "enroll", id: code }); verdict(code); return token; },
    register: async () => { record({ kind: "register" }); return true; },
    resend: async () => { record({ kind: "resend" }); return true; },
    close: async () => {
      record({ kind: "close" });
      // Mimics storage access denied while signing out or deleting the app.
      if (sessionStorage.getItem("fixture-close-error")) throw new Error("storage denied");
    },
  };
}`,
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
        mfa_mandatory: mfaMandatory,
      },
    }),
  );
  await page.route("**/auth/login", (route) => {
    const request = route.request();
    expect(request.method()).toBe("POST");
    const form = new URLSearchParams(request.postData() ?? "");
    if (form.get("email")?.endsWith("@acme.test"))
      return route.fulfill({
        status: 303,
        headers: { location: "/sign-in?state=browser-bound-state" },
        body: "",
      });
    const query = new URLSearchParams({ error: "not_found" });
    query.set("return_to", form.get("return_to") ?? "/");
    return route.fulfill({
      status: 303,
      headers: { location: `/sign-in?${query}` },
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
