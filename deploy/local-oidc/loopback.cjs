// Keep the public loopback OIDC issuer reachable from the backend container.
const net = require("node:net");
const server = net.createServer((client) => {
  const upstream = net.connect(8080, "keycloak");
  client.on("error", () => upstream.destroy());
  upstream.on("error", () => client.destroy());
  client.on("close", () => upstream.destroy());
  upstream.on("close", () => client.destroy());
  client.pipe(upstream);
  upstream.pipe(client);
});
server.listen(18099, "127.0.0.1", () =>
  console.log("OIDC loopback relay ready"),
);
const shutdown = () => server.close(() => process.exit(0));
process.on("SIGTERM", shutdown);
process.on("SIGINT", shutdown);
