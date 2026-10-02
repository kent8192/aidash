// A separate process stands in for the system browser's fixture consent page.
// The OS browser handler and real Google consent are not exercised here.
import assert from "node:assert/strict";
const url = new URL(process.argv[2]);
assert(JSON.parse(process.env.AIDASH_E2E_ORIGINS).includes(url.origin));
assert.equal(url.pathname, "/auth/desktop/authorize");
const response = await fetch(url, {
  redirect: "manual",
  signal: AbortSignal.timeout(10000),
});
assert.equal(response.status, 302);
const callback = new URL(response.headers.get("location"));
assert.equal(callback.protocol, "http:");
assert.equal(callback.hostname, "127.0.0.1");
assert.equal(callback.pathname, "/callback");
assert(callback.port);
assert(
  (
    await fetch(callback, {
      redirect: "error",
      signal: AbortSignal.timeout(10000),
    })
  ).ok,
);
