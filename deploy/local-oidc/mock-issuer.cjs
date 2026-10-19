// A generic OpenID Connect issuer for local development only. It signs in
// whoever names a subject on its form: Authorization Code with S256 PKCE, an
// RS256 ID token and no account administration, like any standard issuer.
const crypto = require("node:crypto");
const http = require("node:http");

const port = Number(process.env.MOCK_OIDC_PORT ?? 18099);
const issuer = process.env.MOCK_OIDC_ISSUER ?? `http://127.0.0.1:${port}`;
const clientId = process.env.MOCK_OIDC_CLIENT_ID ?? "aidash-local";
const clientSecret =
  process.env.MOCK_OIDC_CLIENT_SECRET ?? "aidash-local-secret";

const { privateKey, publicKey } = crypto.generateKeyPairSync("rsa", {
  modulusLength: 2048,
});
const jwk = {
  ...publicKey.export({ format: "jwk" }),
  use: "sig",
  alg: "RS256",
  kid: "local",
};
const codes = new Map();

const base64url = (value) => Buffer.from(value).toString("base64url");
const escape = (value) =>
  String(value).replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);

function idToken(code) {
  const now = Math.floor(Date.now() / 1000);
  const header = base64url(JSON.stringify({ alg: "RS256", kid: jwk.kid }));
  const claims = base64url(
    JSON.stringify({
      iss: issuer,
      aud: clientId,
      sub: code.subject,
      nonce: code.nonce,
      iat: now,
      exp: now + 600,
      email: `${code.subject}@example.test`,
      email_verified: true,
      name: code.subject,
    }),
  );
  const signature = crypto.sign(
    "sha256",
    Buffer.from(`${header}.${claims}`),
    privateKey,
  );
  return `${header}.${claims}.${base64url(signature)}`;
}

function respond(response, status, body, headers = {}) {
  const text = typeof body === "string" ? body : JSON.stringify(body);
  response.writeHead(status, {
    "content-type":
      typeof body === "string"
        ? "text/html; charset=utf-8"
        : "application/json",
    "cache-control": "no-store",
    ...headers,
  });
  response.end(text);
}

function read(request) {
  return new Promise((resolve) => {
    let body = "";
    request.on("data", (chunk) => (body += chunk));
    request.on("end", () => resolve(new URLSearchParams(body)));
  });
}

function client(request, form) {
  const basic = request.headers.authorization?.match(/^Basic (.+)$/);
  if (basic) {
    const [id, secret] = Buffer.from(basic[1], "base64").toString().split(":");
    return [decodeURIComponent(id), decodeURIComponent(secret ?? "")];
  }
  return [form.get("client_id"), form.get("client_secret")];
}

const server = http.createServer(async (request, response) => {
  const url = new URL(request.url, issuer);
  if (
    request.method === "GET" &&
    url.pathname === "/.well-known/openid-configuration"
  ) {
    return respond(response, 200, {
      issuer,
      authorization_endpoint: `${issuer}/authorize`,
      token_endpoint: `${issuer}/token`,
      jwks_uri: `${issuer}/certs`,
      response_types_supported: ["code"],
      subject_types_supported: ["public"],
      id_token_signing_alg_values_supported: ["RS256"],
      token_endpoint_auth_methods_supported: [
        "client_secret_basic",
        "client_secret_post",
      ],
      code_challenge_methods_supported: ["S256"],
    });
  }
  if (request.method === "GET" && url.pathname === "/certs") {
    return respond(response, 200, { keys: [jwk] });
  }
  if (request.method === "GET" && url.pathname === "/authorize") {
    const query = url.searchParams;
    if (
      query.get("client_id") !== clientId ||
      query.get("code_challenge_method") !== "S256"
    ) {
      return respond(response, 400, { error: "invalid_request" });
    }
    const hidden = [...query]
      .map(
        ([name, value]) =>
          `<input type="hidden" name="${escape(name)}" value="${escape(value)}">`,
      )
      .join("");
    return respond(
      response,
      200,
      `<!doctype html><title>Local OIDC issuer</title><form method="post">${hidden}` +
        `<label>Subject <input name="subject" value="local-user" required></label> ` +
        `<button>Sign in</button></form>`,
    );
  }
  if (request.method === "POST" && url.pathname === "/authorize") {
    const form = await read(request);
    const code = crypto.randomBytes(24).toString("base64url");
    codes.set(code, {
      subject: form.get("subject"),
      nonce: form.get("nonce"),
      challenge: form.get("code_challenge"),
      redirect: form.get("redirect_uri"),
    });
    const redirect = new URL(form.get("redirect_uri"));
    redirect.searchParams.set("code", code);
    redirect.searchParams.set("state", form.get("state"));
    return respond(response, 303, "", { location: redirect.toString() });
  }
  if (request.method === "POST" && url.pathname === "/token") {
    const form = await read(request);
    const [id, secret] = client(request, form);
    const code = codes.get(form.get("code"));
    codes.delete(form.get("code"));
    const verifier = form.get("code_verifier") ?? "";
    if (
      id !== clientId ||
      secret !== clientSecret ||
      !code ||
      form.get("grant_type") !== "authorization_code" ||
      form.get("redirect_uri") !== code.redirect ||
      crypto.createHash("sha256").update(verifier).digest("base64url") !==
        code.challenge
    ) {
      return respond(response, 400, { error: "invalid_grant" });
    }
    return respond(response, 200, {
      access_token: crypto.randomBytes(24).toString("base64url"),
      token_type: "Bearer",
      expires_in: 600,
      id_token: idToken(code),
    });
  }
  respond(response, 404, { error: "not_found" });
});

server.listen(port, "0.0.0.0", () =>
  console.log(`Local OIDC issuer ready at ${issuer}`),
);
const shutdown = () => server.close(() => process.exit(0));
process.on("SIGTERM", shutdown);
process.on("SIGINT", shutdown);
