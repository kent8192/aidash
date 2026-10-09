import { createServer } from "node:http";
import { expect, test } from "@playwright/test";
import { mockGcip } from "../tests/fixtures/gcip";

test("GCIP browser sign-in returns through the existing desktop consent and loopback callback", async ({
  page,
}) => {
  let callback;
  const server = createServer((request, response) => {
    callback = request.url;
    response.end("Desktop callback received");
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  try {
    const redirect = `http://127.0.0.1:${server.address().port}/callback`;
    const consent = "/auth/desktop/consent?handoff=fixture";
    await mockGcip(page, consent);
    await page.route("**/auth/desktop/consent?**", (route) =>
      route.fulfill({
        contentType: "text/html",
        body: `<h1>Continue to Aidash Desktop</h1><a href="${redirect}?state=desktop-state&code=one-use-pkce-code">Continue to Aidash Desktop</a>`,
      }),
    );
    await page.goto(`/sign-in?return_to=${encodeURIComponent(consent)}`);
    await page.getByLabel("Organization").fill("acme");
    const login = page.waitForRequest("**/auth/login?**");
    await page.getByRole("button", { name: "Continue", exact: true }).click();
    expect(new URL((await login).url()).searchParams.get("return_to")).toBe(
      consent,
    );
    await page
      .getByRole("button", { name: "Continue with Google", exact: true })
      .click();
    await expect(page).toHaveURL(/\/auth\/desktop\/consent\?handoff=fixture$/);
    await page
      .getByRole("link", { name: "Continue to Aidash Desktop" })
      .click();
    await expect(page).toHaveURL(
      `${redirect}?state=desktop-state&code=one-use-pkce-code`,
    );
    expect(callback).toBe(
      "/callback?state=desktop-state&code=one-use-pkce-code",
    );
  } finally {
    await new Promise((resolve) => {
      server.close(resolve);
      server.closeAllConnections();
    });
  }
});
