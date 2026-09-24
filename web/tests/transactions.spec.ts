import { test, expect } from "@playwright/test";
import {
  installBearerDashboard,
  selectDashboardLanguage,
} from "./auth-fixture";
import { createServer } from "node:http";
import { randomUUID } from "node:crypto";
import type { AddressInfo } from "node:net";

test("transaction dashboard survives reload during a partition, aborts safely and displays committed changes", async ({
  page,
  request,
  baseURL,
}) => {
  test.setTimeout(90000);
  const token = "acceptance-access-token";
  const headers = { authorization: `Bearer ${token}` };
  const peerId = `aidash://transaction-ui-${randomUUID()}`;
  const api = async (path: string, body?: unknown) => {
    const response =
      body === undefined
        ? await request.get(path, { headers })
        : await request.post(path, { headers, data: body });
    expect(response.status(), await response.text()).toBe(200);
    return response.json();
  };
  const peer = createServer(async (req, res) => {
    if (req.url === "/.well-known/aidash") {
      res.writeHead(200, { "content-type": "application/json" });
      res.end(JSON.stringify({ id: peerId, protocol_version: "0.1" }));
      return;
    }
    if (req.url === "/federation/v0.1/transactions/finish") {
      const chunks = [];
      for await (const chunk of req) chunks.push(chunk);
      const manifest = JSON.parse(Buffer.concat(chunks).toString());
      const details = await fetch(
        `${baseURL}/api/transactions/${manifest.id}`,
        { headers },
      ).then((response) => response.json());
      if (details.transaction.decision !== "ABORT") {
        res.writeHead(409);
        res.end();
        return;
      }
      res.writeHead(200, { "content-type": "application/json" });
      res.end(
        JSON.stringify({
          id: manifest.id,
          coordinator: manifest.coordinator,
          digest: details.transaction.digest,
          manifest,
          phase: "ABORTED",
          updated_at: new Date().toISOString(),
        }),
      );
      return;
    }
    res.writeHead(503, { "content-type": "application/json" });
    res.end(JSON.stringify({ error: "fixture partition" }));
  });
  await new Promise<void>((resolve) => peer.listen(0, "127.0.0.1", resolve));
  let pending: string | undefined;
  try {
    const session = await api("/api/session");
    const workspace = await api("/api/workspaces", {
      title: "Atomic dashboard",
      goal: "Commit or abort together",
    });
    await api("/api/peers", {
      node_id: peerId,
      endpoint: `http://127.0.0.1:${(peer.address() as AddressInfo).port}`,
      credential_env: "AIDASH_SECRET_TRANSACTION_FIXTURE",
      protocol_version: "0.1",
      enabled: true,
    });
    await installBearerDashboard(page, "acceptance-access-token");
    await page.addInitScript(() => {
      localStorage.setItem("aidash-locale", "en-US");
    });
    await page.goto("/transactions");
    await page.getByLabel("Peer node", { exact: true }).selectOption(peerId);
    await page
      .getByRole("button", { name: "Grant trust", exact: true })
      .click();
    await expect(
      page.getByRole("button", { name: "Revoke trust", exact: true }),
    ).toBeVisible();
    const manifest = (remote: boolean, expected_revision = 0) => ({
      id: randomUUID(),
      coordinator: session.node_id,
      isolation: "serializable",
      deadline: new Date(Date.now() + 300000).toISOString(),
      participants: [
        {
          node_id: session.node_id,
          mutations: [
            {
              kind: "workspace_state",
              workspace_id: workspace.id,
              expected_revision,
              state: { result: "committed" },
            },
          ],
        },
        ...(remote ? [{ node_id: peerId, mutations: [] }] : []),
      ].sort((a, b) => a.node_id.localeCompare(b.node_id)),
    });
    const submit = async (value: ReturnType<typeof manifest>) => {
      await page
        .getByRole("button", { name: "Create transaction", exact: true })
        .click();
      const dialog = page.getByRole("dialog");
      const mutation = value.participants.find(
        (p) => p.node_id === session.node_id,
      )!.mutations[0];
      await dialog
        .getByLabel("Workspace", { exact: true })
        .selectOption(workspace.id);
      await dialog
        .getByLabel("Workspace state", { exact: true })
        .fill(JSON.stringify(mutation.state));
      await dialog
        .getByLabel("Revision", { exact: true })
        .fill(String(mutation.expected_revision));
      if (value.participants.length > 1) {
        await dialog
          .getByRole("button", { name: "Add participant", exact: true })
          .click();
        await dialog
          .getByLabel("Node", { exact: true })
          .nth(1)
          .selectOption(peerId);
      }
      await dialog
        .getByRole("button", { name: "Review changes", exact: true })
        .click();
      await expect(dialog).toContainText("Atomic dashboard");
      const posted = page.waitForRequest(
        (request) =>
          request.url().endsWith("/api/transactions") &&
          request.method() === "POST",
      );
      await dialog
        .getByRole("button", { name: "Submit transaction", exact: true })
        .click();
      Object.assign(value, (await posted).postDataJSON());
      pending = value.id;
      await expect(page.locator(".transaction-details")).toBeVisible();
      await expect(page.locator(".transaction-details")).not.toContainText(
        value.id,
      );
    };
    const blocked = manifest(true);
    pending = blocked.id;
    await submit(blocked);
    await expect
      .poll(async () =>
        (
          await request.get(`/api/workspaces/${workspace.id}`, { headers })
        ).status(),
      )
      .toBe(503);
    await page.reload();
    await expect(
      page.getByRole("heading", { name: "Transactions", exact: true }),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Create transaction", exact: true }),
    ).toBeVisible();
    await expect(page.getByRole("alert")).toContainText(
      "atomic transaction visibility pending",
    );
    const blockedDeadline = await page.evaluate(
      (deadline) => new Date(deadline).toLocaleString(),
      blocked.deadline,
    );
    await page.getByRole("button").filter({ hasText: blockedDeadline }).click();
    await expect(page.locator(".transaction-details")).toContainText(
      "Preparing",
    );
    const detailResponses: Array<{
      status: number;
      complete?: boolean;
      decision?: string;
    }> = [];
    page.on("response", async (response) => {
      if (
        new URL(response.url()).pathname !== `/api/transactions/${blocked.id}`
      )
        return;
      const body = await response.json().catch(() => undefined);
      detailResponses.push({
        status: response.status(),
        complete: body?.transaction?.complete,
        decision: body?.transaction?.decision,
      });
    });
    await page
      .getByRole("button", { name: "Abort undecided transaction", exact: true })
      .click();
    await expect
      .poll(
        async () =>
          (await api(`/api/transactions/${blocked.id}`)).transaction.complete,
        { timeout: 30000 },
      )
      .toBe(true);
    try {
      await expect(page.locator(".transaction-details > .badge")).toHaveText(
        "Aborted",
      );
    } catch (error) {
      throw new Error(
        `${String(error)}; browser detail responses: ${JSON.stringify(detailResponses)}`,
      );
    }
    expect(
      (await api(`/api/workspaces/${workspace.id}`)).workspace.state,
    ).toEqual({});
    pending = undefined;
    await page.getByRole("button", { name: "Close", exact: true }).click();
    await page
      .getByRole("button", { name: "Revoke trust", exact: true })
      .click();
    await expect(page.getByText("Disabled", { exact: true })).toBeVisible();

    const committed = manifest(false);
    await submit(committed);
    await expect(page.locator(".transaction-details > .badge")).toHaveText(
      "Committed",
    );
    await expect(
      page.getByRole("button", {
        name: "Abort undecided transaction",
        exact: true,
      }),
    ).toHaveCount(0);
    expect(
      (await api(`/api/workspaces/${workspace.id}`)).workspace.state,
    ).toEqual({
      result: "committed",
    });
    await page.screenshot({
      path: "../.ignore/dashboard-transactions-desktop.png",
      fullPage: true,
    });
    await page.getByRole("button", { name: "Close", exact: true }).click();
    await submit(manifest(false));
    await expect(page.locator(".transaction-details > .badge")).toHaveText(
      "Aborted",
    );
    expect(
      (await api(`/api/workspaces/${workspace.id}`)).workspace.revision,
    ).toBe(1);
    await page.getByRole("button", { name: "Close", exact: true }).click();
    await selectDashboardLanguage(page, "ja-JP");
    await page.setViewportSize({ width: 390, height: 844 });
    await expect(
      page.getByRole("heading", { name: "分散トランザクション", exact: true }),
    ).toBeVisible();
    await page.screenshot({
      path: "../.ignore/dashboard-transactions-mobile.png",
      fullPage: true,
    });
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= window.innerWidth,
      ),
    ).toBe(true);
  } finally {
    if (pending) {
      await request.post(`/api/transactions/${pending}/abort`, { headers });
      await expect
        .poll(
          async () =>
            (await api(`/api/transactions/${pending}`)).transaction.complete,
        )
        .toBe(true);
    }
    await new Promise<void>((resolve, reject) => {
      peer.close((error) => (error ? reject(error) : resolve()));
      // The assertions have finished. Close task-owned keep-alive sockets too;
      // background peer requests must not keep fixture teardown alive.
      peer.closeAllConnections();
    });
  }
});
