/* global document, window */
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { createServer } from "node:http";
import { chromium } from "playwright";

// Rust supplies a real production router and a seeded, valid browser session.
const origin = process.argv[2];
const callbacks = [];
const servers = [];
let browser;
try {
  for (let i = 0; i < 2; i++) {
    const server = createServer((request, response) => {
      callbacks.push({
        method: request.method,
        url: request.url,
        referer: request.headers.referer,
      });
      response.end("Desktop callback received");
    });
    servers.push(server);
    await new Promise((resolve, reject) => {
      server.once("error", reject);
      server.listen(0, "127.0.0.1", resolve);
    });
  }
  browser = await chromium.launch();
  const context = await browser.newContext();
  await context.addCookies([
    { name: "aidash-session", value: "desktop-browser-fixture", url: origin },
    { name: "aidash-csrf", value: "desktop-csrf", url: origin },
  ]);
  const page = await context.newPage();
  for (const [index, server] of servers.entries()) {
    const callbackOrigin = `http://127.0.0.1:${server.address().port}`;
    const redirect = `${callbackOrigin}/callback`;
    const state = String(index + 1).repeat(64);
    const verifier = "v".repeat(64);
    const started = await fetch(`${origin}/auth/desktop/start`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        redirect_uri: redirect,
        state,
        code_challenge: createHash("sha256")
          .update(verifier)
          .digest("base64url"),
      }),
    });
    assert.equal(started.status, 200);
    const { authorization_url: authorization } = await started.json();
    const consent = await page.goto(authorization);
    assert.equal(consent.status(), 200);
    assert.equal(
      await page.locator('meta[name="referrer"]').getAttribute("content"),
      "same-origin",
    );
    assert.equal(
      consent.headers()["content-security-policy"],
      `default-src 'none'; form-action 'self' ${callbackOrigin}; frame-ancestors 'none'; base-uri 'none'`,
    );
    const approval = page.waitForResponse(
      (response) =>
        new URL(response.url()).pathname === "/auth/desktop/authorize" &&
        response.request().method() === "POST",
    );
    await page
      .getByRole("button", { name: "Continue to Aidash Desktop" })
      .click({ noWaitAfter: true });
    const approved = await approval;
    assert.equal((await approved.request().allHeaders()).origin, origin);
    assert.equal(approved.status(), 303);
    await page.waitForURL(`${callbackOrigin}/callback?**`);
    assert.equal(callbacks.length, index + 1);
    const callback = new URL(callbacks[index].url, callbackOrigin);
    assert.equal(callbacks[index].method, "GET");
    assert.equal(callbacks[index].referer, undefined);
    assert.equal(callback.pathname, "/callback");
    assert.equal(callback.searchParams.get("state"), state);
    assert.ok(callback.searchParams.get("code"));
    const exchanged = await fetch(`${origin}/auth/desktop/exchange`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        code: callback.searchParams.get("code"),
        state,
        verifier,
        redirect_uri: redirect,
      }),
    });
    assert.equal(exchanged.status, 200);

    // A different loopback port must remain blocked by this request's policy.
    const another = await fetch(`${origin}/auth/desktop/start`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        redirect_uri: redirect,
        state,
        code_challenge: createHash("sha256")
          .update(verifier)
          .digest("base64url"),
      }),
    });
    assert.equal(another.status, 200);
    await page.goto((await another.json()).authorization_url);
    const otherOrigin = `http://127.0.0.1:${servers[1 - index].address().port}`;
    await page.evaluate((otherOrigin) => {
      window.violations = [];
      document.addEventListener("securitypolicyviolation", (event) => {
        window.violations.push(event.effectiveDirective);
      });
      document.querySelector("form").action = `${otherOrigin}/callback`;
    }, otherOrigin);
    await page
      .getByRole("button", { name: "Continue to Aidash Desktop" })
      .click({ noWaitAfter: true });
    await page.waitForFunction(() => window.violations.includes("form-action"));
    assert.equal(callbacks.length, index + 1);
  }
  console.log(
    "Chromium production consent: two exact-port callbacks and unrelated-port denial passed",
  );
} finally {
  await browser?.close();
  await Promise.all(
    servers.map(
      (server) =>
        new Promise((resolve) => {
          server.close(resolve);
          server.closeAllConnections();
        }),
    ),
  );
}
