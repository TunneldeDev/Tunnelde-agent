import { createInterface } from "node:readline";

if (process.argv.slice(2).join(" ") !== "--initialize-config")
  throw new Error("Explicit initialized child transport is required");
const pending = new Map();
let sequence = 100, configuration, secrets;
const send = message => process.stdout.write(JSON.stringify(message) + "\n");
const call = (method, params) => new Promise((resolve, reject) => {
  const id = ++sequence;
  pending.set(id, { resolve, reject });
  send({ jsonrpc: "2.0", id, method, params });
});
createInterface({ input: process.stdin }).on("line", async line => {
  const message = JSON.parse(line);
  if (!message.method) {
    const request = pending.get(message.id);
    pending.delete(message.id);
    if (message.error) request?.reject(new Error(message.error.message));
    else request?.resolve(message.result);
    return;
  }
  try {
    let result;
    if (message.method === "initialize") {
      if (message.params.configurationTransport !== "initialize-v1")
        throw new Error("Missing initialized configuration");
      configuration = message.params.configuration;
      secrets = message.params.secrets;
      await call("events.publish", { event: "ready", payload: { initialized: true } });
      result = { ok: true };
    } else {
      const original = await call("storage.get", { key: "config" });
      await call("storage.set", { key: "ordinary", value: "allowed persistence" });
      result = {
        configurationReceived: configuration?.private === "initialized-private-configuration",
        secretReceived: secrets?.token === "initialized-private-token",
        hostSecretReceived: await call("secrets.get", { key: "token" }) === "initialized-private-token",
        configurationSeed: original,
        argv: process.argv.slice(2),
      };
    }
    send({ jsonrpc: "2.0", id: message.id, result });
  } catch (error) {
    send({ jsonrpc: "2.0", id: message.id, error: { code: -32000, message: error.message } });
  }
});
