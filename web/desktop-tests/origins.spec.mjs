/* global window, document */
import { readFile } from "node:fs/promises";
import { createServer } from "node:http";
import { test, expect } from "@playwright/test";

// The routed harness has no network address-space classification. Grant the
// browser's loopback permission while leaving the production CSP enforced.
test.use({ permissions: ["local-network-access"] });

const config = JSON.parse(
  await readFile(
    new URL("../../desktop/src-tauri/tauri.conf.json", import.meta.url),
    "utf8",
  ),
);

for (const [host, bind] of [
  ["127.0.0.1", "127.0.0.1"],
  ["localhost", "127.0.0.1"],
  ["localhost", "::1"],
]) {
  test(`desktop CSP and transport can contact HTTP ${host} on ${bind}`, async ({
    page,
  }) => {
    const requests = [];
    const server = createServer((request, response) => {
      requests.push(request.url);
      response.writeHead(200, {
        "access-control-allow-origin": "http://127.0.0.1:18083",
        "content-type": "application/json",
      });
      response.end(JSON.stringify({ enabled: true, desktop_protocol: 1 }));
    });
    try {
      await new Promise((resolve, reject) => {
        server.once("error", reject);
        server.listen(0, bind, resolve);
      });
      await page.route("**/origin-harness", (route) =>
        route.fulfill({
          contentType: "text/html",
          headers: { "content-security-policy": config.app.security.csp },
          body: "<!doctype html><title>Desktop origin harness</title>",
        }),
      );
      await page.goto("/origin-harness");
      const result = await page.evaluate(async (origin) => {
        window.__TAURI_INTERNALS__ = { invoke: async () => null };
        const transport = await import("/src/transport.ts");
        transport.selectConnection({
          id: "loopback",
          name: "Loopback",
          origin,
        });
        const violations = [];
        document.addEventListener("securitypolicyviolation", (event) => {
          violations.push(event.effectiveDirective);
        });
        const response = await transport.sessionFetch("/auth/config");
        return {
          status: response.status,
          body: await response.json(),
          violations,
        };
      }, `http://${host}:${server.address().port}`);
      expect(result).toEqual({
        status: 200,
        body: { enabled: true, desktop_protocol: 1 },
        violations: [],
      });
      expect(requests).toEqual(["/auth/config"]);
    } finally {
      await new Promise((resolve) => {
        server.close(resolve);
        server.closeAllConnections();
      });
    }
  });
}
