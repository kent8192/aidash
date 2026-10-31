import assert from "node:assert/strict";
import { test } from "node:test";
import { authorizePopup } from "../src/connections/popup.ts";
import {
  scopeIntersection,
  canonicalResource,
  resourceKey,
} from "../src/connections/scopes.ts";

test("scope choices respect every present bound and preserve an empty restriction", () => {
  assert.deepEqual(
    scopeIntersection(
      ["write", "read", "read", "admin"],
      ["read", "write"],
      ["read"],
    ),
    ["read"],
  );
  assert.deepEqual(scopeIntersection(undefined, ["write", "read"], ["write"]), [
    "write",
  ]);
  assert.deepEqual(scopeIntersection(["read"], null, null), ["read"]);
  assert.deepEqual(scopeIntersection(undefined, undefined, ["read"]), ["read"]);
  assert.deepEqual(scopeIntersection(["read"], [], undefined), []);
  assert.deepEqual(scopeIntersection(["read"], undefined, []), []);
  assert.deepEqual(scopeIntersection([], ["read"], ["read"]), []);
  assert.deepEqual(scopeIntersection(), []);
});

test("scope policy paths use the canonical resource encoded as unpadded base64url", () => {
  const resource = canonicalResource("https://MCP.example:443/mcp/");
  assert.equal(resource, "https://mcp.example/mcp");
  assert.equal(
    Buffer.from(resourceKey(resource), "base64url").toString(),
    resource,
  );
  for (const value of [
    "https://mcp.example?token=x",
    "https://u:p@mcp.example",
    "https://mcp.example#x",
    "http://mcp.example",
    "http://127.attacker.example",
    "https://@mcp.example",
    "https://mcp.example?",
    "https://mcp.example#",
  ])
    assert.equal(canonicalResource(value), null);
  assert.equal(
    canonicalResource("http://127.0.0.2:8765/mcp"),
    "http://127.0.0.2:8765/mcp",
  );
});

function browser(t, blocked = false) {
  const previous = globalThis.window;
  const listeners = new Set();
  const navigations = [];
  const popup = {
    closed: false,
    close() {
      this.closed = true;
    },
    location: {
      replace(url) {
        navigations.push(["popup", url]);
      },
    },
  };
  globalThis.window = {
    location: {
      origin: "https://aidash.example",
      assign(url) {
        navigations.push(["page", url]);
      },
    },
    open() {
      return blocked ? null : popup;
    },
    addEventListener(name, listener) {
      if (name === "message") listeners.add(listener);
    },
    removeEventListener(name, listener) {
      if (name === "message") listeners.delete(listener);
    },
    setInterval,
    clearInterval,
  };
  t.after(() => {
    globalThis.window = previous;
  });
  return {
    popup,
    navigations,
    listeners,
    message(overrides = {}) {
      for (const listener of listeners)
        listener({
          origin: "https://aidash.example",
          source: popup,
          data: { type: "aidash.connection", outcome: "connected" },
          ...overrides,
        });
    },
  };
}

test("a popup accepts only its own Aidash-origin completion and removes its listener", async (t) => {
  const b = browser(t);
  const result = authorizePopup(async () => "https://issuer.example/authorize");
  await Promise.resolve();
  assert.deepEqual(b.navigations, [
    ["popup", "https://issuer.example/authorize"],
  ]);
  b.message({ origin: "https://attacker.example" });
  b.message({ source: {} });
  b.message({ data: { type: "other", outcome: "connected" } });
  b.message({ data: { type: "aidash.connection", outcome: "unexpected" } });
  assert.equal(b.popup.closed, false);
  assert.equal(b.listeners.size, 1);
  b.message();
  assert.equal(await result, "connected");
  assert.equal(b.popup.closed, true);
  assert.equal(b.listeners.size, 0);
});

test("closing the popup settles the flow even while authorization is loading", async (t) => {
  const b = browser(t);
  let started;
  const result = authorizePopup(
    () =>
      new Promise((resolve) => {
        started = resolve;
      }),
  );
  b.popup.closed = true;
  assert.equal(await result, "closed");
  started("https://issuer.example/authorize");
  await Promise.resolve();
  assert.deepEqual(b.navigations, []);
  assert.equal(b.listeners.size, 0);
});

test("a blocked popup falls back to full-page authorization", async (t) => {
  const b = browser(t, true);
  assert.equal(
    await authorizePopup(async () => "https://issuer.example/authorize"),
    "redirected",
  );
  assert.deepEqual(b.navigations, [
    ["page", "https://issuer.example/authorize"],
  ]);
  assert.equal(b.listeners.size, 0);
});

test("a failed start closes the blank popup and releases the listener", async (t) => {
  const b = browser(t);
  await assert.rejects(
    authorizePopup(async () => {
      throw new Error("offline");
    }),
    /offline/,
  );
  assert.equal(b.popup.closed, true);
  assert.equal(b.listeners.size, 0);
});
