import { expect, test, type Page } from "@playwright/test";
import { setup } from "./collaboration-fixture";

const personalMapping = {
  id: "personal-mapping",
  tenant: "acme",
  subject: "alice",
  personal: true,
  credential_active: true,
};

async function installSession(
  page: Page,
  options: {
    locale?: "ja-JP" | "en-US";
    operator?: boolean;
    personalAccount?: boolean;
    mappings?: (typeof personalMapping)[];
    selected?: string;
  } = {},
) {
  await setup(page, {
    subject: !options.operator,
    locale: options.locale,
    dashboardContext: options.selected ?? null,
  });
  await page.addInitScript(() => {
    document.cookie = "aidash-csrf=personal-csrf; path=/";
  });
  await page.route("**/auth/session", (route) =>
    route.fulfill({
      json: {
        id: "fixture-browser-session",
        operator: options.operator ?? false,
        personal_account: options.personalAccount ?? true,
        mappings: options.mappings ?? [personalMapping],
      },
    }),
  );
}

for (const [locale, provider, label] of [
  ["ja-JP", "google", "Google でサインイン"],
  ["en-US", "google", "Sign in with Google"],
  ["en-US", "keycloak", "Sign in with Keycloak"],
]) {
  test(`${provider} login in ${locale} preserves the destination`, async ({
    page,
  }) => {
    await page.addInitScript(
      (locale) => localStorage.setItem("aidash-locale", locale),
      locale,
    );
    await page.route("**/auth/config", (route) =>
      route.fulfill({
        json: { enabled: true, provider, login_url: "/auth/login" },
      }),
    );
    await page.route("**/auth/session", (route) =>
      route.fulfill({ status: 401, body: "" }),
    );
    await page.route("**/auth/login?**", (route) =>
      route.fulfill({
        contentType: "text/plain",
        body: "OAuth redirect fixture",
      }),
    );
    await page.goto("/settings");
    await page.getByRole("button", { name: label, exact: true }).click();
    await expect(page).toHaveURL(/\/auth\/login\?return_to=%2Fsettings$/);
  });
}

for (const locale of ["ja-JP", "en-US"] as const) {
  test(`personal sign-in starts immediately and shared approval stays separate in ${locale}`, async ({
    page,
  }) => {
    await installSession(page, { locale });
    const ja = locale === "ja-JP";
    let registration: { status: string; expires_at: string } | null = null;
    let requests = 0;
    await page.route("**/auth/registration", (route) => {
      const request = route.request();
      if (request.method() === "POST") {
        requests += 1;
        expect(request.headers()["x-aidash-csrf"]).toBe("personal-csrf");
        expect(request.headers().origin).toBe(new URL(request.url()).origin);
        registration = {
          status: "pending",
          expires_at: "2099-01-01T00:00:00Z",
        };
      }
      return route.fulfill({ json: registration });
    });
    const selected: string[] = [];
    await page.route("**/api/session", (route) => {
      selected.push(route.request().headers()["x-aidash-context"]);
      return route.fallback();
    });
    await page.goto("/collaboration");
    const account = page.getByRole("button", {
      name: ja ? "アカウント設定" : "Account settings",
      exact: true,
    });
    await expect(account).toBeVisible();
    expect(selected).toContain("mapping:personal-mapping");
    expect(requests).toBe(0);
    await account.click();
    await expect(page.locator("option[value=operator]")).toHaveCount(0);
    await page
      .getByRole("button", {
        name: ja
          ? "共有テナントへの参加を申請"
          : "Request access to shared tenant",
        exact: true,
      })
      .click();
    await expect(
      page.getByText(
        ja
          ? "共有テナントへの参加申請は承認待ちです。個人用テナントは引き続き利用できます。"
          : "Your shared tenant request is awaiting approval. You can continue using your personal tenant.",
        { exact: true },
      ),
    ).toBeVisible();
    expect(requests).toBe(1);
    await page.reload();
    await expect(account).toBeVisible();
    await account.click();
    await expect(
      page.getByRole("combobox", {
        name: ja
          ? "このタブで使う権限を選択してください"
          : "Choose the authority for this tab",
      }),
    ).toHaveValue("mapping:personal-mapping");
    expect(requests).toBe(1);
  });
}

for (const selected of ["mapping:shared-mapping", "operator", null]) {
  test(`personal onboarding ${selected ? `preserves explicitly selected ${selected}` : "defaults to personal access with an Operator grant"}`, async ({
    page,
  }) => {
    await installSession(page, {
      operator: true,
      selected: selected ?? undefined,
      mappings: [
        personalMapping,
        {
          id: "shared-mapping",
          tenant: "team",
          subject: "alice",
          personal: false,
          credential_active: true,
        },
      ],
    });
    await page.route("**/api/session", (route) => {
      expect(route.request().headers()["x-aidash-context"]).toBe(
        selected ?? "mapping:personal-mapping",
      );
      return route.fallback();
    });
    await page.goto("/collaboration");
    await page
      .getByRole("button", { name: "Account settings", exact: true })
      .click();
    await expect(
      page.getByRole("combobox", {
        name: "Choose the authority for this tab",
      }),
    ).toHaveValue(selected ?? "mapping:personal-mapping");
  });
}

test("a denied shared request keeps personal access and reports the failure", async ({
  page,
}) => {
  await installSession(page);
  await page.route("**/auth/registration", (route) =>
    route.request().method() === "POST"
      ? route.fulfill({ status: 403, json: { error: "Request rejected" } })
      : route.fulfill({ json: null }),
  );
  await page.goto("/collaboration");
  await page
    .getByRole("button", { name: "Account settings", exact: true })
    .click();
  const request = page.getByRole("button", {
    name: "Request access to shared tenant",
    exact: true,
  });
  await request.click();
  await expect(page.getByRole("alert")).toHaveText("Request rejected");
  await expect(request).toBeEnabled();
  await expect(
    page.getByRole("combobox", {
      name: "Choose the authority for this tab",
    }),
  ).toHaveValue("mapping:personal-mapping");
});

test("legacy mappings still require an explicit authority choice", async ({
  page,
}) => {
  await installSession(page, {
    personalAccount: false,
    mappings: [{ ...personalMapping, personal: false }],
  });
  await page.goto("/collaboration");
  await expect(
    page.getByRole("heading", {
      name: "Choose the authority for this tab",
    }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "acme / alice", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "operator", exact: true }),
  ).toHaveCount(0);
  await page.getByRole("button", { name: "acme / alice", exact: true }).click();
  await page
    .getByRole("button", { name: "Account settings", exact: true })
    .click();
  await expect(
    page.getByRole("button", {
      name: "Request access to shared tenant",
      exact: true,
    }),
  ).toHaveCount(0);
});

test("a revoked personal credential is not selected automatically", async ({
  page,
}) => {
  await installSession(page, {
    operator: true,
    mappings: [
      { ...personalMapping, credential_active: false },
      {
        id: "shared-mapping",
        tenant: "team",
        subject: "alice",
        personal: false,
        credential_active: true,
      },
    ],
  });
  const selected: string[] = [];
  await page.route("**/api/session", (route) => {
    selected.push(route.request().headers()["x-aidash-context"]);
    return route.fallback();
  });
  await page.goto("/collaboration");
  await expect(
    page.getByRole("heading", {
      name: "Choose the authority for this tab",
    }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "team / alice", exact: true }),
  ).toBeVisible();
  expect(selected).toEqual([]);
  await page.getByRole("button", { name: "team / alice", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "Account settings", exact: true }),
  ).toBeVisible();
  expect(selected).toContain("mapping:shared-mapping");
  expect(selected).not.toContain("mapping:personal-mapping");
});

test("a saved personal selection with an inactive credential returns to the chooser", async ({
  page,
}) => {
  await installSession(page, {
    operator: true,
    selected: "mapping:personal-mapping",
    mappings: [
      { ...personalMapping, credential_active: false },
      {
        id: "shared-mapping",
        tenant: "team",
        subject: "alice",
        personal: false,
        credential_active: true,
      },
    ],
  });
  const selected: string[] = [];
  await page.route("**/api/session", (route) => {
    selected.push(route.request().headers()["x-aidash-context"]);
    return route.fallback();
  });
  await page.goto("/collaboration");
  await expect(
    page.getByRole("heading", {
      name: "Choose the authority for this tab",
    }),
  ).toBeVisible();
  expect(selected).toEqual([]);
  await page.getByRole("button", { name: "operator", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "Account settings", exact: true }),
  ).toBeVisible();
  expect(selected).toContain("operator");
  expect(selected).not.toContain("mapping:personal-mapping");
});

test("a disabled personal mapping keeps the shared-access request", async ({
  page,
}) => {
  await installSession(page, {
    selected: "mapping:shared-mapping",
    mappings: [
      {
        id: "shared-mapping",
        tenant: "team",
        subject: "alice",
        personal: false,
        credential_active: true,
      },
    ],
  });
  let registration: { status: string; expires_at: string } | null = null;
  await page.route("**/auth/registration", (route) => {
    if (route.request().method() === "POST")
      registration = {
        status: "pending",
        expires_at: "2099-01-01T00:00:00Z",
      };
    return route.fulfill({ json: registration });
  });
  await page.goto("/collaboration");
  await page
    .getByRole("button", { name: "Account settings", exact: true })
    .click();
  await page
    .getByRole("button", {
      name: "Request access to shared tenant",
      exact: true,
    })
    .click();
  await expect(
    page.getByText("Your registration is awaiting approval.", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("combobox", {
      name: "Choose the authority for this tab",
    }),
  ).toHaveValue("mapping:shared-mapping");
});
