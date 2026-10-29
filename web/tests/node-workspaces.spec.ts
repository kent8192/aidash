import { randomUUID } from "node:crypto";
import { readFile, rename, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { expect, test } from "@playwright/test";
import { installBearerDashboard } from "./auth-fixture";
import { acceptanceSubject } from "./tenant-fixture";
import type { ChannelAttachment, NodeRecord } from "../src/generated/models";

// Only a Tenant Administrator adds Nodes, and only a browser Mapping can be one
// (#484, ADR 0102): neither the Deployment Administrator nor a Bearer subject
// can, so the live stack has no way to create the second Node yet.
const administrator = { authorization: "Bearer acceptance-access-token" };

test("a Deployment Administrator cannot add or list a Tenant's Nodes", async ({
  request,
}) => {
  const subject = acceptanceSubject();
  const added = await request.post(`/api/tenants/${subject.tenant}/nodes`, {
    headers: administrator,
    data: { label: `Lab ${randomUUID().slice(0, 8)}` },
  });
  expect(added.status(), await added.text()).toBe(403);
  const listed = await request.get(`/api/tenants/${subject.tenant}/nodes`, {
    headers: administrator,
  });
  expect(listed.status(), await listed.text()).toBe(403);
  const own = await request.post(`/api/tenants/${subject.tenant}/nodes`, {
    headers: { authorization: `Bearer ${subject.token}` },
    data: { label: "Bearer Lab" },
  });
  expect(own.status(), await own.text()).toBe(403);
});

test("an isolated Node retains real attachment bytes until a normal worker purges it through the DA panel", async ({
  page,
  request,
}, testInfo) => {
  test.setTimeout(120000);
  const contextPath = process.env.AIDASH_E2E_NODE_LIFECYCLE_FIXTURE;
  if (!contextPath)
    throw new Error(
      "Run this owner through scripts/test-acceptance.sh --browser-suite",
    );
  const fixture: {
    tenant: string;
    target: string;
    sibling: string;
    subject: { name: string; token: string };
    administrator: Record<string, string>;
  } = JSON.parse(await readFile(contextPath, "utf8"));
  // Default, deletion and restore are Tenant administration (ADR 0102): the
  // fixture's Tenant Administrator acts through its browser Mapping headers.
  // The Deployment Administrator only reads deleted Nodes and purges (ADR 0083).
  const tenantAdministrator = fixture.administrator;
  const tenantPath = `/api/tenants/${fixture.tenant}/nodes`;
  const managementPath = `${tenantPath}?include_deleted=true&offset=0&limit=25`;
  const subject = { authorization: `Bearer ${fixture.subject.token}` };
  const nodes = async (): Promise<NodeRecord[]> => {
    const response = await request.get(managementPath, {
      headers: administrator,
    });
    expect(response.status()).toBe(200);
    return response.json();
  };
  const before = await nodes();
  const targetBefore = before.find((node) => node.name === fixture.target)!;
  const siblingBefore = before.find((node) => node.name === fixture.sibling)!;
  expect(targetBefore.state).toBe("live");
  expect(siblingBefore.default).toBe(true);
  expect(targetBefore.retained_storage_bytes).toBe(0);
  expect(siblingBefore.retained_storage_bytes).toBe(0);

  // The ordinary Subject uses genuine HTTP credentials, not a browser Mapping.
  // These bytes are actually persisted in channel_attachments, not a mock file.
  const store = async (node: string, content: Buffer) => {
    const created = await request.post("/api/workspaces", {
      headers: subject,
      data: {
        title: `Lifecycle ${node}`,
        goal: "Retain and recover evidence",
        node,
      },
    });
    expect(created.status()).toBe(200);
    const workspace: { id: string; node: string } = await created.json();
    expect(workspace.node).toBe(node);
    const query = new URLSearchParams({
      filename: "lifecycle-evidence.bin",
      media_type: "application/octet-stream",
      idempotency_key: randomUUID(),
    });
    const uploaded = await request.post(
      `/api/workspaces/${workspace.id}/attachments?${query}`,
      {
        headers: { ...subject, "content-type": "application/octet-stream" },
        data: content,
      },
    );
    expect(uploaded.status()).toBe(200);
    const attachment: ChannelAttachment = await uploaded.json();
    expect(attachment.size_bytes).toBe(content.byteLength);
    const bound = await request.post(
      `/api/workspaces/${workspace.id}/thread-messages`,
      {
        headers: subject,
        data: {
          content: "Independent retained evidence",
          idempotency_key: randomUUID(),
          attachment_ids: [attachment.id],
        },
      },
    );
    expect(bound.status()).toBe(200);
    const path = `/api/workspaces/${workspace.id}/attachments/${attachment.id}`;
    const downloaded = await request.get(path, { headers: subject });
    expect(downloaded.status()).toBe(200);
    expect(await downloaded.body()).toEqual(content);
    return { workspace: workspace.id, path };
  };
  const targetBytes = Buffer.from(
    "Target evidence retained across deletion.\n",
  );
  const siblingBytes = Buffer.from(
    "Independent sibling evidence must survive.\n",
  );
  const targetData = await store(fixture.target, targetBytes);
  const siblingData = await store(fixture.sibling, siblingBytes);
  const measured = await nodes();
  expect(
    measured.find((node) => node.name === fixture.target)!
      .retained_storage_bytes,
  ).toBe(targetBytes.byteLength);
  expect(
    measured.find((node) => node.name === fixture.sibling)!
      .retained_storage_bytes,
  ).toBe(siblingBytes.byteLength);

  // Existing UI session shell only: all /api calls retain real DA authority.
  // No Subject Mapping, actual OIDC login or additional authority is supplied.
  await installBearerDashboard(page, "acceptance-access-token");
  await page.addInitScript(() =>
    localStorage.setItem("aidash-locale", "en-US"),
  );
  const openManagement = async () => {
    const initial = page.waitForResponse(
      (response) =>
        new URL(response.url()).pathname === "/api/tenants" &&
        response.request().method() === "GET",
    );
    await page.goto("/trust?view=authorization");
    expect((await initial).status()).toBe(200);
    const tenants = page
      .getByRole("heading", { name: "Tenants", exact: true })
      .locator("..");
    const first = tenants.getByRole("listitem").first();
    await expect(first).toBeVisible();
    const row = tenants
      .getByRole("listitem")
      .filter({ hasText: fixture.tenant });
    while ((await row.count()) === 0) {
      const previous = await first.innerText();
      const next = tenants.getByRole("button", { name: "Next", exact: true });
      await expect(next).toBeEnabled();
      const loaded = page.waitForResponse(
        (response) =>
          new URL(response.url()).pathname === "/api/tenants" &&
          response.request().method() === "GET",
      );
      await next.click();
      await loaded;
      await expect(first).not.toHaveText(previous);
    }
    await row
      .getByRole("button", { name: "Manage Nodes", exact: true })
      .click();
    await expect(
      page.locator(`[data-lifecycle-node="${fixture.target}"]`),
    ).toContainText(
      `Stored data: ${targetBytes.byteLength.toLocaleString("en-US")} bytes`,
    );
  };
  await openManagement();
  const target = page.locator(`[data-lifecycle-node="${fixture.target}"]`);
  const mutation = (node: string, action: string) =>
    page.waitForResponse(
      (response) =>
        new URL(response.url()).pathname ===
          `${tenantPath}/${node}/${action}` &&
        response.request().method() === "POST",
    );
  const administer = async (node: string, action: string, data?: object) => {
    const response = await request.post(`${tenantPath}/${node}/${action}`, {
      headers: tenantAdministrator,
      data,
    });
    expect(response.status(), await response.text()).toBe(200);
    return (await response.json()) as NodeRecord;
  };
  // The panel offers the Deployment Administrator no Tenant administration.
  await expect(
    target.getByRole("button", { name: "Make default", exact: true }),
  ).toHaveCount(0);
  await expect(
    target.getByRole("button", { name: "Delete Node", exact: true }),
  ).toHaveCount(0);
  for (const action of ["default", "deletion", "restore"]) {
    const refused = await request.post(
      `${tenantPath}/${fixture.target}/${action}`,
      {
        headers: administrator,
        data:
          action === "deletion" ? { confirmation: fixture.target } : undefined,
      },
    );
    expect(refused.status(), await refused.text()).toBe(403);
  }
  expect((await administer(fixture.sibling, "default")).default).toBe(true);

  const remove = async () => {
    const record = await administer(fixture.target, "deletion", {
      confirmation: fixture.target,
    });
    expect(record.state).toBe("deleted");
    expect(record.retained_storage_bytes).toBe(targetBytes.byteLength);
    await openManagement();
    return record;
  };
  const deleted = await remove();
  const retained = (await nodes()).find(
    (node) => node.name === fixture.target,
  )!;
  expect(retained.deletion?.purge_after).toBe(deleted.deletion!.purge_after);
  expect(retained.retained_storage_bytes).toBe(targetBytes.byteLength);
  await expect(target).toContainText(
    `Retained data: ${targetBytes.byteLength.toLocaleString("en-US")} bytes`,
  );
  const live = await request.get(tenantPath, { headers: subject });
  expect(live.status()).toBe(200);
  const liveNodes: NodeRecord[] = await live.json();
  expect(liveNodes.map((node) => node.name)).toEqual([fixture.sibling]);
  expect(liveNodes[0].retained_storage_bytes).toBeNull();
  expect(
    (await request.get(targetData.path, { headers: subject })).status(),
  ).toBe(404);
  const fallback = await request.post("/api/workspaces", {
    headers: subject,
    data: {
      title: "Live default fallback",
      goal: "Never select the deleted Node",
    },
  });
  expect(fallback.status()).toBe(200);
  expect((await fallback.json()).node).toBe(fixture.sibling);

  await expect(
    target.getByRole("button", { name: "Restore Node", exact: true }),
  ).toHaveCount(0);
  expect((await administer(fixture.target, "restore")).state).toBe("live");
  const recovered = await request.get(targetData.path, { headers: subject });
  expect(recovered.status()).toBe(200);
  expect(await recovered.body()).toEqual(targetBytes);
  const secondDeletion = await remove();
  await target
    .getByRole("button", { name: "Purge Node now", exact: true })
    .click();
  const purgeDialog = page.getByRole("dialog", {
    name: "Purge Node permanently",
    exact: true,
  });
  await expect(purgeDialog).toContainText(
    `Retained data: ${targetBytes.byteLength.toLocaleString("en-US")} bytes`,
  );
  await purgeDialog
    .getByLabel("Type the Node name to confirm")
    .fill(fixture.target);
  const purging = mutation(fixture.target, "purge");
  await purgeDialog
    .getByRole("button", { name: "Purge Node permanently", exact: true })
    .click();
  const accepted = await purging;
  expect(accepted.status()).toBe(200);
  const requested: NodeRecord = await accepted.json();
  expect(requested.state).toBe("deleting");
  expect(requested.deletion!.id).toBe(secondDeletion.deletion!.id);
  expect(requested.deletion!.purge_after).toBe(
    secondDeletion.deletion!.purge_after,
  );

  // Atomic test-only handoff; Python verifies the same committed cycle using
  // its own API and retains DB/key/settings before launching one normal worker.
  const handoff = join(dirname(contextPath), "purge-request.json");
  await writeFile(
    `${handoff}.tmp`,
    JSON.stringify({
      tenant: fixture.tenant,
      node: fixture.target,
      deletion_id: requested.deletion!.id,
      expected_bytes: targetBytes.byteLength,
    }),
    { mode: 0o600 },
  );
  await rename(`${handoff}.tmp`, handoff);
  await expect
    .poll(
      async () =>
        (await nodes()).find((node) => node.name === fixture.target)?.state,
      { timeout: 60000 },
    )
    .toBe("purged");
  const purged = (await nodes()).find((node) => node.name === fixture.target)!;
  expect(purged.retained_storage_bytes).toBe(0);
  expect(purged.deletion?.purged_at).toBeTruthy();
  expect(purged.deletion?.purge_after).toBe(
    secondDeletion.deletion!.purge_after,
  );
  await expect(target).toContainText("Purged");
  await expect(target).toContainText("Stored data: 0 bytes");
  // Workspace ownership lookup returns Forbidden; attachment download conceals
  // it as NotFound. The sibling read proves this Subject's authority stays live.
  expect(
    (
      await request.get(`/api/workspaces/${targetData.workspace}`, {
        headers: subject,
      })
    ).status(),
  ).toBe(403);
  expect(
    (await request.get(targetData.path, { headers: subject })).status(),
  ).toBe(404);
  const preserved = await request.get(siblingData.path, { headers: subject });
  expect(preserved.status()).toBe(200);
  expect(await preserved.body()).toEqual(siblingBytes);
  expect(
    (await nodes()).find((node) => node.name === fixture.sibling)!
      .retained_storage_bytes,
  ).toBe(siblingBytes.byteLength);
  const evidencePath = join(dirname(contextPath), "worker-evidence.json");
  await expect
    .poll(
      async () => {
        const evidence = await readFile(evidencePath, "utf8")
          .then(JSON.parse)
          .catch(() => null);
        return (
          evidence?.worker_stopped === true && evidence?.state === "purged"
        );
      },
      { timeout: 15000 },
    )
    .toBe(true);
  await testInfo.attach("normal-worker-purge", {
    path: evidencePath,
    contentType: "application/json",
  });
});

// FIXME(#484): needs a Tenant Administrator Mapping (or a host-side Node
// command) in the live stack to create the second Node.
test.fixme(
  "a Workspace created in a second Node belongs to that Node and to no other",
  async ({ page, request }) => {
    // A Tenant subject's state can take seconds to refresh (#404).
    test.setTimeout(120000);
    const subject = acceptanceSubject();
    const label = `Lab ${randomUUID().slice(0, 8)}`;
    const added = await request.post(`/api/tenants/${subject.tenant}/nodes`, {
      headers: { authorization: `Bearer ${subject.token}` },
      data: { label },
    });
    expect(added.status(), await added.text()).toBe(201);
    const second = (await added.json()) as { name: string; label: string };
    const nodes = (await (
      await request.get(`/api/tenants/${subject.tenant}/nodes`, {
        headers: { authorization: `Bearer ${subject.token}` },
      })
    ).json()) as { name: string; label: string; default: boolean }[];
    const home = nodes.find((node) => node.default)!;
    expect(home.name).not.toBe(second.name);

    await installBearerDashboard(page, subject.token, {
      tenant: subject.tenant,
      name: subject.name,
    });
    await page.addInitScript(() =>
      localStorage.setItem("aidash-locale", "en-US"),
    );
    await page.goto("/collaboration");
    await page.getByRole("button", { name: "Account settings" }).click();
    const selector = page.getByTestId("node-selector");
    await expect(selector).toHaveValue(home.name);
    await selector.selectOption(second.name);
    await page.keyboard.press("Escape");

    const title = `Second Node workspace ${Date.now()}`;
    await page.getByRole("button", { name: "Settings", exact: true }).click();
    await page.getByRole("button", { name: "Prepare a channel" }).click();
    const dialog = page.getByRole("dialog");
    await expect(dialog.getByLabel("Node", { exact: true })).toHaveValue(
      second.name,
    );
    await dialog.getByLabel("Title", { exact: true }).fill(title);
    await dialog
      .getByLabel("Goal", { exact: true })
      .fill("Live in the Lab Node");
    const previousChannel = new URL(page.url()).searchParams.get("channel");
    await dialog.getByRole("button", { name: "Create", exact: true }).click();
    await expect(page.getByRole("dialog")).not.toBeVisible({ timeout: 30000 });
    await expect(page).toHaveURL((url) => {
      const channel = url.searchParams.get("channel");
      return !!channel && channel !== previousChannel;
    });
    const workspaceId = new URL(page.url()).searchParams.get("channel");
    expect(workspaceId).toBeTruthy();

    const read = await request.get(`/api/workspaces/${workspaceId}`, {
      headers: { authorization: `Bearer ${subject.token}` },
    });
    expect(read.status(), await read.text()).toBe(200);
    expect(
      ((await read.json()) as { workspace: { node: string; title: string } })
        .workspace,
    ).toMatchObject({ node: second.name, title });

    const listed = () =>
      page
        .getByRole("button", { name: /^Switch request/ })
        .click()
        .then(() => page.getByRole("navigation", { name: "Workspaces" }));
    await expect(await listed()).toContainText(title);
    await page.keyboard.press("Escape");

    // The default Node does not list it.
    await page.getByRole("button", { name: "Account settings" }).click();
    await page.getByTestId("node-selector").selectOption(home.name);
    await page.keyboard.press("Escape");
    await expect(await listed()).not.toContainText(title);
  },
);
