import { createServer } from "node:http";
import { randomBytes, randomUUID, createHash } from "node:crypto";
import { meshScene } from "../../../web/tests/mesh-scene.mjs";

const secret = () => randomBytes(32).toString("base64url");

/** Synthetic broker/API only. No Google account or deployment secrets are used. */
export async function startFixture() {
  const scene = meshScene();
  const pending = new Map();
  const codes = new Map();
  const refresh = new Set();
  let access;
  let origin;
  const stats = {
    logins: 0,
    renewals: 0,
    revocations: 0,
    requests: [],
    streams: [],
  };
  const server = createServer(async (req, res) => {
    const reply = (status, value, headers = {}, body) => {
      const requestOrigin = req.headers.origin;
      res.writeHead(status, {
        "Content-Type": "application/json",
        "Cache-Control": "no-store",
        ...(requestOrigin === "tauri://localhost" ||
        requestOrigin === "http://tauri.localhost"
          ? { "Access-Control-Allow-Origin": requestOrigin }
          : {}),
        "Access-Control-Allow-Headers":
          "authorization,content-type,x-aidash-context,last-event-id",
        "Access-Control-Allow-Methods": "GET,POST,OPTIONS",
        ...headers,
      });
      res.end(status === 204 ? undefined : (body ?? JSON.stringify(value)));
    };
    try {
      const url = new URL(req.url, origin);
      if (req.method === "OPTIONS") return reply(204);
      if (url.pathname === "/_test/status")
        return reply(200, { ...stats, active: !!access });
      if (req.method === "POST") {
        const chunks = [];
        for await (const chunk of req) chunks.push(chunk);
        const input = JSON.parse(Buffer.concat(chunks).toString() || "{}");
        const tokens = (token) => {
          access = `aidash_desktop_${secret()}`;
          refresh.add(token);
          reply(200, {
            access_token: access,
            refresh_token: token,
            expires_in: 300,
          });
        };
        if (url.pathname === "/auth/desktop/start") {
          const id = randomUUID();
          pending.set(id, input);
          return reply(200, {
            authorization_url: `${origin}/auth/desktop/authorize?request=${id}`,
          });
        }
        if (url.pathname === "/auth/desktop/exchange") {
          const handoff = codes.get(input.code);
          codes.delete(input.code);
          if (
            !handoff ||
            handoff.state !== input.state ||
            handoff.redirect_uri !== input.redirect_uri ||
            handoff.code_challenge !==
              createHash("sha256").update(input.verifier).digest("base64url")
          ) {
            return reply(401, {});
          }
          stats.logins++;
          return tokens(`aidash_refresh_${secret()}`);
        }
        if (url.pathname === "/auth/desktop/refresh") {
          if (!refresh.delete(input.refresh_token)) return reply(401, {});
          stats.renewals++;
          return tokens(input.next_token);
        }
        if (url.pathname === "/auth/desktop/revoke") {
          if (!refresh.has(input.refresh_token)) return reply(401, {});
          stats.revocations++;
          refresh.clear();
          access = undefined;
          return reply(204);
        }
        if (url.pathname === "/auth/activity") return reply(204);
        return reply(404, {});
      }
      if (url.pathname === "/auth/config") {
        return reply(200, {
          enabled: true,
          provider: "google",
          desktop_protocol: 1,
        });
      }
      if (url.pathname === "/auth/desktop/authorize") {
        const id = url.searchParams.get("request");
        const handoff = pending.get(id);
        pending.delete(id);
        if (!handoff) return reply(400, {});
        const code = secret();
        codes.set(code, handoff);
        const callback = new URL(handoff.redirect_uri);
        callback.search = new URLSearchParams({ code, state: handoff.state });
        return reply(302, {}, { Location: callback.href });
      }
      stats.requests.push({
        path: url.pathname,
        cookie: req.headers.cookie ?? null,
        context: req.headers["x-aidash-context"] ?? null,
      });
      if (!access || req.headers.authorization !== `Bearer ${access}`)
        return reply(401, {});
      switch (url.pathname) {
        case "/auth/session":
          return reply(200, {
            id: `session-${server.address().port}`,
            operator: true,
            mappings: [],
          });
        case "/auth/registration":
          return reply(200, null);
        case "/api/session":
          return reply(200, {
            access: { kind: "operator" },
            node_id: scene.data.node.id,
          });
        case "/api/state":
          return reply(200, scene.data);
        case "/api/mesh":
          return reply(200, { nodes: [], errors: [] });
        case "/api/discover":
          return reply(200, scene.discovery);
        case "/api/events/stream": {
          stats.streams.push(req.headers["last-event-id"] ?? null);
          return reply(
            200,
            null,
            { "Content-Type": "text/event-stream" },
            `id: ${server.address().port}\ndata: {}\n\n`,
          );
        }
        case "/api/workspaces/product-lab":
          return reply(200, {
            workspace: scene.data.workspaces[0],
            tasks: scene.data.tasks,
            artifacts: scene.data.artifacts,
            events: scene.data.events,
            messages: [],
          });
        default:
          return reply(
            200,
            url.pathname.endsWith("/message-history")
              ? { messages: [], next_before: null }
              : [],
          );
      }
    } catch {
      // Never print requests: auth bodies and callback URLs contain credentials.
      reply(500, { error: "fixture request failed" });
    }
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  origin = `http://127.0.0.1:${server.address().port}`;
  return {
    origin,
    stats,
    close: () =>
      new Promise((resolve) => {
        server.close(resolve);
        server.closeAllConnections();
      }),
  };
}
