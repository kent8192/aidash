/* global window, document, history, location, innerHeight, describe, before, it */
import assert from "node:assert/strict";
import { resolve } from "node:path";
import { browser, $ } from "@wdio/globals";

const origins = JSON.parse(process.env.AIDASH_E2E_ORIGINS);
const phase = process.env.AIDASH_E2E_PHASE;
const status = async (index) =>
  (await fetch(`${origins[index]}/_test/status`)).json();

async function textContains(value) {
  await browser.waitUntil(
    async () => (await $("body").getText()).includes(value),
    {
      timeoutMsg: `Expected screen: ${value}`,
    },
  );
}
async function button(text) {
  const element = await $(`button=${text}`);
  await element.waitForClickable();
  await element.click();
}
// The connection bar and form carry accessible names rather than CSS hooks.
const connections = '[aria-label="Aidash connections"]';
async function select(name) {
  // Native <option> clicks differ across embedded WebDriver implementations.
  // Dispatch the select's change event to exercise the real React/native path.
  await browser.execute(
    (bar, label) => {
      const select = document.querySelector(
        `${bar} select[aria-label="Connection"]`,
      );
      const option = [...select.options].find(
        (item) => item.textContent === label,
      );
      if (!option) throw new Error(`Missing connection: ${label}`);
      select.value = option.value;
      select.dispatchEvent(new Event("change", { bubbles: true }));
    },
    connections,
    name,
  );
}
async function add(name, origin) {
  await select("Manage connections");
  await textContains("Connect to Aidash");
  await $("main form input:not([type=url])").setValue(name);
  await $("main form input[type=url]").setValue(origin);
  await button("Save and connect");
}
async function authorize() {
  await button("Sign in with Google");
  await button("operator");
  // The navigation rail names its destinations through aria-label only.
  await $('button[aria-label="Graph View"]').waitForExist();
}
async function invoke(command, args = {}) {
  return browser.executeAsync(
    (commandName, commandArgs, done) => {
      window.__TAURI_INTERNALS__.invoke(commandName, commandArgs).then(
        (value) => done({ value }),
        (error) => done({ rejected: String(error) }),
      );
    },
    command,
    args,
  );
}
async function logout() {
  await $('button[aria-label="Account settings"]').click();
  await button("Log out on this device");
  await textContains("Sign in with Google");
}

describe(`Native desktop ${phase} (${process.platform})`, () => {
  before(async () => {
    await browser.tauri.switchWindow("main");
    await browser.execute(() => {
      localStorage.setItem("aidash-locale", "en-US");
      history.replaceState({}, "", "/graph?channel=product-lab");
    });
    await browser.refresh();
  });

  if (phase === "login") {
    it("opens bundled UI, rejects unsafe origins and signs in through the external fixture", async () => {
      await textContains("Connect to Aidash");
      assert.match(
        (
          await invoke("save_connection", {
            name: "Unsafe",
            origin: "http://example.com",
          })
        ).rejected,
        /HTTPS/,
      );
      await add("First", origins[0]);
      await authorize();
      await browser.waitUntil(async () =>
        (await status(0)).streams.includes(new URL(origins[0]).port),
      );
      assert.equal((await status(0)).streams[0], "-1");
    });

    it("renders the real Graph canvas and labels and fits inside the desktop window", async () => {
      await $(".mesh-canvas canvas").waitForExist();
      await $(".mesh-node-label").waitForExist();
      await button("Fit entire graph");
      const geometry = await browser.execute(
        (bar) => ({
          canvases: [...document.querySelectorAll(".mesh-canvas canvas")].map(
            (canvas) => ({ width: canvas.width, height: canvas.height }),
          ),
          labels: document.querySelectorAll(".mesh-node-label").length,
          // The connection bar's parent is the desktop shell around the app.
          bottom: document
            .querySelector(bar)
            .parentElement.getBoundingClientRect().bottom,
          viewport: innerHeight,
        }),
        connections,
      );
      assert(
        geometry.canvases.length > 0 &&
          geometry.canvases.every(
            (canvas) => canvas.width > 0 && canvas.height > 0,
          ),
      );
      assert(geometry.labels > 0);
      assert(
        geometry.bottom <= geometry.viewport + 1,
        JSON.stringify(geometry),
      );
      await browser.saveScreenshot(
        resolve("artifacts/wdio", process.platform, "graph.png"),
      );
    });

    it("switches connection authority, resets SSE cursors and preserves each login", async () => {
      await add("Second", origins[1]);
      await authorize();
      await browser.waitUntil(
        async () => (await status(1)).streams.length >= 2,
      );
      const second = await status(1);
      assert.equal(second.streams[0], "-1");
      assert(second.streams.includes(new URL(origins[1]).port));
      assert(!second.streams.includes(new URL(origins[0]).port));
      const oldStreams = (await status(0)).streams.length;
      await select(`First · ${origins[0]}`);
      await button("operator");
      await browser.waitUntil(
        async () => (await status(0)).streams.length > oldStreams,
      );
      assert.equal((await status(0)).streams[oldStreams], "-1");
      for (const index of [0, 1]) {
        const fixture = await status(index);
        assert.equal(fixture.logins, 1);
        assert(fixture.requests.every((request) => !request.cookie));
      }
    });

    it("denies remote navigation and generic native IPC without exposing stored credentials", async () => {
      const before = await browser.getUrl();
      await browser.execute((origin) => {
        location.href = `${origin}/untrusted`;
      }, origins[1]);
      // Let the native navigation callback run before checking both URL and UI.
      await browser.pause(300);
      assert.equal(await browser.getUrl(), before);
      await $(connections).waitForExist();
      assert(
        (await invoke("plugin:opener|open_url", { url: origins[1] })).rejected,
      );
      assert.equal(
        await browser.execute(() =>
          Object.values(localStorage).some((value) =>
            /aidash_(refresh|desktop)_/.test(value),
          ),
        ),
        false,
      );
    });
  } else if (phase === "restore") {
    it("restores both OS-stored logins in a new native process and revokes them on logout", async () => {
      await button("operator");
      const first = await status(0);
      assert.equal(first.logins, 1);
      assert(first.renewals >= 1);
      await select(`Second · ${origins[1]}`);
      await button("operator");
      const second = await status(1);
      assert.equal(second.logins, 1);
      assert(second.renewals >= 1);
      await logout();
      assert.equal((await status(1)).active, false);
      await select(`First · ${origins[0]}`);
      await button("operator");
      await logout();
      assert.equal((await status(0)).active, false);
    });
  } else if (phase === "logged-out") {
    it("stays logged out after another native process restart", async () => {
      await textContains("Sign in with Google");
      assert.deepEqual(await invoke("desktop_access"), { value: null });
      await select(`Second · ${origins[1]}`);
      await textContains("Sign in with Google");
      assert.deepEqual(await invoke("desktop_access"), { value: null });
      for (const index of [0, 1]) {
        const fixture = await status(index);
        assert.equal(fixture.logins, 1);
        assert.equal(fixture.revocations, 1);
      }
    });
  } else {
    throw new Error(`Unknown acceptance phase: ${phase}`);
  }
});
