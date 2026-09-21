import { test, expect } from "@playwright/test";
import { createServer } from "node:http";
import type { AddressInfo } from "node:net";

test("semantic dashboard configures, searches, migrates and deletes persistent sources in both languages", async ({
  page,
  request,
}) => {
  test.setTimeout(90000);
  const headers = { authorization: "Bearer acceptance-access-token" };
  const api = async (path: string, data?: unknown) => {
    const response =
      data === undefined
        ? await request.get(path, { headers })
        : await request.post(path, { headers, data });
    expect(response.status(), await response.text()).toBe(200);
    return response.json();
  };
  const server = createServer(async (req, res) => {
    const chunks: Buffer[] = [];
    for await (const chunk of req) chunks.push(Buffer.from(chunk));
    const input = JSON.parse(Buffer.concat(chunks).toString());
    const vector = /car|vehicle/i.test(input.input) ? [1, 0, 0] : [0, 1, 0];
    res.writeHead(200, { "content-type": "application/json" });
    res.end(
      JSON.stringify({
        model: input.model,
        data: [{ index: 0, embedding: vector }],
      }),
    );
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const endpoint = `http://127.0.0.1:${(server.address() as AddressInfo).port}/v1`;
  const workspace = await api("/api/workspaces", {
    title: "Semantic dashboard",
    goal: "Retrieve authorized context",
  });
  const root = `/api/workspaces/${workspace.id}/semantic`;
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  let releaseRefresh = () => {};
  let releaseConfigure = () => {};
  try {
    await page.addInitScript(() => {
      sessionStorage.setItem("aidash-token", "acceptance-access-token");
      localStorage.setItem("aidash-locale", "en-US");
    });
    await page.goto("/semantic");
    await page
      .getByLabel("Workspace", { exact: true })
      .selectOption(workspace.id);
    await page
      .getByRole("button", { name: "Configure index", exact: true })
      .click();
    await page
      .getByLabel("Embedding API endpoint", { exact: true })
      .fill(endpoint);
    await page
      .getByLabel("Embedding model", { exact: true })
      .fill("semantic-fixture");
    await page.getByLabel("Model version", { exact: true }).fill("v1");
    await page.getByLabel("Vector dimensions", { exact: true }).fill("3");
    await page
      .getByLabel("Qdrant endpoint", { exact: true })
      .fill(process.env.AIDASH_TEST_QDRANT_URL ?? "http://127.0.0.1:63370");
    await page.getByRole("button", { name: "Save", exact: true }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await page
      .getByRole("button", { name: "Add or edit source", exact: true })
      .click();
    await page.getByLabel("Source key", { exact: true }).fill("transport");
    await page
      .getByLabel("Memory content", { exact: true })
      .fill("A car carries passengers.");
    await page.getByRole("button", { name: "Save", exact: true }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(page.locator(".semantic-entry .badge")).toHaveText("Ready", {
      timeout: 15000,
    });
    // Hold the search's query refresh so the next dialog opens while it is busy.
    const refreshGate = new Promise<void>((resolve) => {
      releaseRefresh = resolve;
    });
    await page.route(
      `**${root}/search`,
      async (searchRoute) => {
        const response = await searchRoute.fetch();
        await page.route(`**${root}/history`, async (route) => {
          const history = await route.fetch();
          await refreshGate;
          await route.fulfill({ response: history });
        });
        await searchRoute.fulfill({ response });
      },
      { times: 1 },
    );
    await page.getByLabel("Search by meaning", { exact: true }).fill("vehicle");
    await page
      .getByRole("button", { name: "Semantic search", exact: true })
      .click();
    await expect(page.locator(".semantic-result")).toContainText(
      "A car carries passengers.",
    );
    await expect(page.locator(".semantic-result")).toContainText("memory");
    await page
      .getByRole("button", { name: "Configure index", exact: true })
      .click();
    await page.getByLabel("Model version", { exact: true }).fill("v2");
    const saveIndex = page.getByRole("dialog").getByRole("button", {
      name: "Save",
      exact: true,
    });
    await expect(saveIndex).toBeDisabled();
    releaseRefresh();
    await expect(saveIndex).toBeEnabled();
    await expect(page.getByLabel("Model version", { exact: true })).toHaveValue(
      "v2",
    );
    let configureStarted = false;
    let holdConfigure = true;
    const configureGate = new Promise<void>((resolve) => {
      releaseConfigure = resolve;
    });
    await page.route(`**${root}/index`, async (route) => {
      if (route.request().method() !== "POST" || !holdConfigure) {
        await route.continue();
        return;
      }
      configureStarted = true;
      const response = await route.fetch();
      await configureGate;
      holdConfigure = false;
      await route.fulfill({ response });
    });
    await saveIndex.click();
    await expect.poll(() => configureStarted).toBe(true);
    await expect(saveIndex).toBeDisabled();
    await page
      .getByRole("dialog")
      .getByRole("button", { name: "Close" })
      .click();
    await page
      .getByRole("button", { name: "Configure index", exact: true })
      .click();
    await page.getByLabel("Model version", { exact: true }).fill("v3");
    releaseConfigure();
    await expect(page.locator(".semantic-summary").first()).toContainText(
      "Revision 2",
    );
    await expect(page.getByRole("dialog")).toBeVisible();
    await expect(page.getByLabel("Model version", { exact: true })).toHaveValue(
      "v3",
    );
    await page
      .getByRole("dialog")
      .getByRole("button", { name: "Close" })
      .click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect
      .poll(async () => (await api(`${root}/entries`))[0].state)
      .toBe("READY");
    await page
      .getByRole("button", { name: "Semantic search", exact: true })
      .click();
    await expect(page.locator('[aria-live="polite"]')).toContainText("v2");
    await page.screenshot({
      path: "../.ignore/dashboard-semantic-desktop.png",
      fullPage: true,
    });
    await page.reload();
    await page
      .getByLabel("Workspace", { exact: true })
      .selectOption(workspace.id);
    await expect(page.locator(".semantic-entry")).toContainText("transport");
    await page.getByRole("button", { name: "Delete", exact: true }).click();
    await page
      .getByRole("dialog")
      .getByRole("button", { name: "Delete", exact: true })
      .click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(page.locator(".semantic-entry")).toHaveCount(0);
    await page.getByLabel("Search by meaning", { exact: true }).fill("vehicle");
    await page
      .getByRole("button", { name: "Semantic search", exact: true })
      .click();
    await expect(
      page.getByText("No authorized matches.", { exact: true }),
    ).toBeVisible();
    await expect
      .poll(async () => {
        const cleanup = await api(`${root}/cleanup`);
        return cleanup.points.pending + cleanup.collections.pending;
      })
      .toBe(0);
    await page.getByLabel("Language", { exact: true }).selectOption("ja-JP");
    await page.setViewportSize({ width: 390, height: 844 });
    await expect(
      page.getByRole("heading", { name: /^セマンティックメモリ/ }),
    ).toBeVisible();
    await page.screenshot({
      path: "../.ignore/dashboard-semantic-mobile.png",
      fullPage: true,
    });
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= window.innerWidth,
      ),
    ).toBe(true);
    expect(errors).toEqual([]);
  } finally {
    releaseRefresh();
    releaseConfigure();
    await new Promise<void>((resolve, reject) =>
      server.close((error) => (error ? reject(error) : resolve())),
    );
    // Delete only this fixture's current collection. Old generations are
    // retired by the running service and retried if acknowledgement was lost.
    const indexResponse = await request.get(`${root}/index`, { headers });
    if (indexResponse.ok()) {
      const index = await indexResponse.json();
      await request.delete(
        `${index.spec.vector.endpoint}/collections/${index.collection}`,
        {
          headers: {
            "api-key":
              process.env.AIDASH_SECRET_TEST_QDRANT ??
              "local-semantic-vector-fixture-key-0123456789",
          },
        },
      );
    }
  }
});
