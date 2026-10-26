import { randomUUID } from "node:crypto";
import { expect, test } from "@playwright/test";
import { installBearerDashboard } from "./auth-fixture";
import { acceptanceSubject } from "./tenant-fixture";

// A Tenant Administrator adds Nodes. A Bearer subject has no Mapping, so it
// cannot administer, and the Deployment Administrator adds the second Node.
const administrator = { authorization: "Bearer acceptance-access-token" };

test("a Workspace created in a second Node belongs to that Node and to no other", async ({
  page,
  request,
}) => {
  // A Tenant subject's state can take seconds to refresh (#404).
  test.setTimeout(120000);
  const subject = acceptanceSubject();
  const label = `Lab ${randomUUID().slice(0, 8)}`;
  const added = await request.post(`/api/tenants/${subject.tenant}/nodes`, {
    headers: administrator,
    data: { label },
  });
  expect(added.status(), await added.text()).toBe(201);
  const second = (await added.json()) as { name: string; label: string };
  const nodes = (await (
    await request.get(`/api/tenants/${subject.tenant}/nodes`, {
      headers: administrator,
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
  await dialog.getByLabel("Goal", { exact: true }).fill("Live in the Lab Node");
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
});
