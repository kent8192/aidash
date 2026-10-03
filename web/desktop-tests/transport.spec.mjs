/* global window, document */
import { test, expect } from "@playwright/test";

test.beforeEach(async ({ page }) => {
  await page.route("**/transport-harness", (route) =>
    route.fulfill({
      contentType: "text/html",
      body: "<!doctype html><title>Transport harness</title>",
    }),
  );
  await page.goto("/transport-harness");
});

test("Web cookies/CSRF and bodyless responses retain their contract", async ({
  page,
}) => {
  const result = await page.evaluate(async () => {
    const transport = await import("/src/transport.ts");
    document.cookie = "aidash-csrf=browser-csrf";
    const calls = [];
    window.fetch = async (url, options) => {
      calls.push({
        url,
        credentials: options.credentials,
        csrf: options.headers.get("x-aidash-csrf"),
        bearer: options.headers.get("authorization"),
      });
      return new Response(null, { status: 204 });
    };
    const response = await transport.sessionFetch("/auth/activity", {
      method: "POST",
    });
    return { calls, status: response.status, body: await response.text() };
  });
  expect(result).toEqual({
    calls: [
      {
        url: "/auth/activity",
        credentials: "same-origin",
        csrf: "browser-csrf",
        bearer: null,
      },
    ],
    status: 204,
    body: "",
  });
});

test("desktop requests bind credentials to the connection and reject stale JSON bodies", async ({
  page,
}) => {
  const result = await page.evaluate(async () => {
    window.__TAURI_INTERNALS__ = {
      invoke: async () => ({
        access_token: "aidash_desktop_fixture",
        expires_in: 300,
      }),
    };
    const transport = await import("/src/transport.ts");
    transport.selectConnection({
      id: "one",
      name: "One",
      origin: "https://one.example",
    });
    transport.selectDashboardContext("operator");
    document.cookie = "aidash-csrf=browser-csrf";
    const calls = [];
    let body;
    window.fetch = async (url, options) => {
      calls.push({
        url,
        credentials: options.credentials,
        redirect: options.redirect,
        csrf: options.headers.get("x-aidash-csrf"),
        bearer: options.headers.get("authorization"),
        context: options.headers.get("x-aidash-context"),
      });
      return new Response(
        new ReadableStream({
          start(controller) {
            body = controller;
          },
        }),
        { headers: { "content-type": "application/json" } },
      );
    };
    const response = await transport.sessionFetch("/api/state", {
      headers: {
        "x-aidash-csrf": "must-remove",
        authorization: "must-replace",
      },
    });
    const pending = response.json().then(
      () => "leaked",
      () => "rejected",
    );
    transport.selectConnection({
      id: "two",
      name: "Two",
      origin: "https://two.example",
    });
    body.enqueue(new TextEncoder().encode('{"private":"old authority"}'));
    body.close();
    const stale = await pending;
    const invalid = await transport
      .sessionFetch("https://evil.example/api/state")
      .then(
        () => "allowed",
        () => "rejected",
      );
    return { calls, stale, invalid, context: transport.dashboardContext() };
  });
  expect(result).toEqual({
    calls: [
      {
        url: "https://one.example/api/state",
        credentials: "omit",
        redirect: "error",
        csrf: null,
        bearer: "Bearer aidash_desktop_fixture",
        context: "operator",
      },
    ],
    stale: "rejected",
    invalid: "rejected",
    context: null,
  });
});

test("authority switching fences late response headers and authentication failures", async ({
  page,
}) => {
  const result = await page.evaluate(async () => {
    const transport = await import("/src/transport.ts");
    transport.selectDashboardContext("mapping:old");
    let release;
    let expired = false;
    window.addEventListener(transport.AUTHENTICATION_EXPIRED, () => {
      expired = true;
    });
    window.fetch = () =>
      new Promise((resolve) => {
        release = resolve;
      });
    const pending = transport.apiFetch("/api/state").then(
      () => "leaked",
      () => "rejected",
    );
    await Promise.resolve();
    transport.selectDashboardContext("mapping:new");
    release(
      new Response('{"error":"expired old identity"}', {
        status: 401,
        headers: { "content-type": "application/json" },
      }),
    );
    return {
      result: await pending,
      expired,
      context: transport.dashboardContext(),
    };
  });
  expect(result).toEqual({
    result: "rejected",
    expired: false,
    context: "mapping:new",
  });
});

test("SSE reconnect sends Last-Event-ID and a new authority starts without the old cursor", async ({
  page,
}) => {
  const result = await page.evaluate(async () => {
    const transport = await import("/src/transport.ts");
    const { subscribe } = await import("/src/event-stream.ts");
    const calls = [];
    let signal = new AbortController();
    let events = 0;
    window.fetch = async (_url, options) => {
      calls.push(options.headers.get("Last-Event-ID"));
      return new Response(`id: ${calls.length * 41}\ndata: {}\n\n`, {
        headers: { "content-type": "text/event-stream" },
      });
    };
    const pending = subscribe(
      signal.signal,
      () => {
        if (++events === 4) signal.abort();
      },
      () => {},
    );
    await pending;
    transport.selectDashboardContext("mapping:another");
    signal = new AbortController();
    const restarted = subscribe(
      signal.signal,
      () => {
        signal.abort();
      },
      () => {},
    );
    await restarted;
    return calls;
  });
  expect(result).toEqual(["-1", "41", "-1"]);
});

test("a revoked access token refreshes once and mutations are never retried after network failure", async ({
  page,
}) => {
  const result = await page.evaluate(async () => {
    const invocations = [];
    window.__TAURI_INTERNALS__ = {
      invoke: async (name, args) => {
        invocations.push({ name, args });
        return {
          access_token: args.rejectedToken ? "new-access" : "old-access",
          expires_in: 300,
        };
      },
    };
    const transport = await import("/src/transport.ts");
    transport.selectConnection({
      id: "one",
      name: "One",
      origin: "https://one.example",
    });
    let sent = 0;
    window.fetch = async () =>
      new Response("{}", { status: ++sent === 1 ? 401 : 200 });
    await transport.apiFetch("/api/state");
    let mutations = 0;
    window.fetch = async () => {
      mutations++;
      throw new TypeError("network lost");
    };
    await transport
      .sessionFetch("/api/tasks", { method: "POST" })
      .catch(() => {});
    return {
      sent,
      mutations,
      rejected: invocations.map((i) => i.args.rejectedToken),
    };
  });
  expect(result).toEqual({
    sent: 2,
    mutations: 1,
    rejected: [null, "old-access", null],
  });
});
