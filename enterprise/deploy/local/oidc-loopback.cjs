// Keep the browser's exact loopback issuer reachable inside the local BFF.
const net = require("node:net");
const server = net.createServer((client) => {
  const upstream = net.connect(8081, "authentication");
  client.on("error", () => upstream.destroy());
  upstream.on("error", () => client.destroy());
  client.on("close", () => upstream.destroy());
  upstream.on("close", () => client.destroy());
  client.pipe(upstream);
  upstream.pipe(client);
});
server.listen(+process.env.AUTH_PORT, "127.0.0.1", () =>
  console.log("Authentication loopback relay ready"),
);
const shutdown = () => server.close(() => process.exit(0));
process.on("SIGTERM", shutdown);
process.on("SIGINT", shutdown);
