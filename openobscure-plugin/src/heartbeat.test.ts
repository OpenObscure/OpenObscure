import { describe, it, afterEach } from "node:test";
import assert from "node:assert/strict";
import * as http from "http";

import {
  HeartbeatMonitor,
  STATE_MESSAGES,
  type ProxyState,
  type HealthResponse,
} from "./heartbeat";

// ── Test helpers ──

/** Create a mock HTTP server returning a health response. */
function createMockServer(
  response: Partial<HealthResponse> | null,
  statusCode: number = 200
): Promise<{ server: http.Server; port: number }> {
  return new Promise((resolve) => {
    const server = http.createServer((_req, res) => {
      if (response === null) {
        // Simulate timeout by not responding
        return;
      }
      res.writeHead(statusCode, { "Content-Type": "application/json" });
      res.end(
        JSON.stringify({
          status: "ok",
          version: "0.1.0",
          uptime_secs: 100,
          pii_matches_total: 5,
          requests_total: 10,
          ...response,
        })
      );
    });
    server.listen(0, "127.0.0.1", () => {
      const addr = server.address() as { port: number };
      resolve({ server, port: addr.port });
    });
  });
}

/** Create a mock HTTP server that requires X-OpenObscure-Token auth. */
function createAuthServer(
  expectedToken: string
): Promise<{ server: http.Server; port: number }> {
  return new Promise((resolve) => {
    const server = http.createServer((req, res) => {
      const token = req.headers["x-openobscure-token"];
      if (token !== expectedToken) {
        res.writeHead(401, { "Content-Type": "application/json" });
        res.end(JSON.stringify({ error: "unauthorized" }));
        return;
      }
      res.writeHead(200, { "Content-Type": "application/json" });
      res.end(
        JSON.stringify({
          status: "ok",
          version: "0.1.0",
          uptime_secs: 100,
          pii_matches_total: 5,
          requests_total: 10,
        })
      );
    });
    server.listen(0, "127.0.0.1", () => {
      const addr = server.address() as { port: number };
      resolve({ server, port: addr.port });
    });
  });
}

let servers: http.Server[] = [];
afterEach(() => {
  for (const s of servers) {
    s.close();
  }
  servers = [];
});

// ── Tests ──

describe("HeartbeatMonitor", () => {
  it("initial state is disabled", () => {
    const monitor = new HeartbeatMonitor();
    assert.equal(monitor.state, "disabled");
    assert.equal(monitor.lastHealth, null);
    assert.equal(monitor.consecutiveFailures, 0);
  });

  it("check() returns health response from healthy proxy", async () => {
    const { server, port } = await createMockServer({});
    servers.push(server);

    const monitor = new HeartbeatMonitor({
      proxyUrl: `http://127.0.0.1:${port}`,
      intervalMs: 60_000,
      timeoutMs: 2_000,
    });
    monitor.start();

    const health = await monitor.check();
    assert.ok(health);
    assert.equal(health!.status, "ok");
    assert.equal(health!.version, "0.1.0");
    assert.equal(health!.uptime_secs, 100);
    assert.equal(health!.pii_matches_total, 5);

    assert.equal(monitor.state, "active");
    assert.equal(monitor.consecutiveFailures, 0);

    monitor.stop();
  });

  it("transitions to degraded when proxy is unreachable", async () => {
    const stateChanges: ProxyState[] = [];
    const monitor = new HeartbeatMonitor({
      proxyUrl: "http://127.0.0.1:1", // Nothing listening
      intervalMs: 60_000,
      timeoutMs: 500,
      onStateChange: (state) => stateChanges.push(state),
    });
    monitor.start();

    await monitor.check();

    assert.equal(monitor.state, "degraded");
    // >= 1 because start() fires an immediate tick that also fails
    assert.ok(monitor.consecutiveFailures >= 1);
    assert.ok(stateChanges.includes("degraded"));

    monitor.stop();
  });

  it("tracks consecutive failures", async () => {
    const monitor = new HeartbeatMonitor({
      proxyUrl: "http://127.0.0.1:1",
      intervalMs: 60_000,
      timeoutMs: 500,
    });
    monitor.start();

    // Wait for the immediate startup tick to settle
    await new Promise((r) => setTimeout(r, 100));
    const baseline = monitor.consecutiveFailures;

    await monitor.check();
    await monitor.check();
    await monitor.check();

    assert.equal(monitor.consecutiveFailures, baseline + 3);

    monitor.stop();
  });

  it("transitions degraded → recovering → active on recovery", async () => {
    const stateChanges: ProxyState[] = [];

    // Start with proxy down
    const monitor = new HeartbeatMonitor({
      proxyUrl: "http://127.0.0.1:1",
      intervalMs: 60_000,
      timeoutMs: 500,
      onStateChange: (state) => stateChanges.push(state),
    });
    monitor.start();
    await monitor.check(); // → degraded

    assert.equal(monitor.state, "degraded");

    // Now start a working server and point monitor at it
    const { server, port } = await createMockServer({});
    servers.push(server);

    // Hack: update the proxyUrl by creating a new monitor
    const monitor2 = new HeartbeatMonitor({
      proxyUrl: `http://127.0.0.1:${port}`,
      intervalMs: 60_000,
      timeoutMs: 2_000,
      onStateChange: (state) => stateChanges.push(state),
    });
    // Simulate degraded state
    monitor2.start();
    // First force degraded
    (monitor2 as any)._state = "degraded";
    (monitor2 as any)._consecutiveFailures = 2;

    await monitor2.check(); // → recovering → active

    assert.equal(monitor2.state, "active");
    assert.equal(monitor2.consecutiveFailures, 0);
    assert.ok(stateChanges.includes("recovering"));

    monitor.stop();
    monitor2.stop();
  });

  it("stop() transitions to disabled", () => {
    const stateChanges: ProxyState[] = [];
    const monitor = new HeartbeatMonitor({
      intervalMs: 60_000,
      onStateChange: (state) => stateChanges.push(state),
    });
    monitor.start();
    assert.equal(monitor.state, "active");

    monitor.stop();
    assert.equal(monitor.state, "disabled");
    assert.ok(stateChanges.includes("disabled"));
  });

  it("handles non-200 status codes as failure", async () => {
    const { server, port } = await createMockServer({}, 503);
    servers.push(server);

    const monitor = new HeartbeatMonitor({
      proxyUrl: `http://127.0.0.1:${port}`,
      intervalMs: 60_000,
      timeoutMs: 2_000,
    });
    monitor.start();

    const health = await monitor.check();
    assert.equal(health, null);
    assert.equal(monitor.state, "degraded");

    monitor.stop();
  });

  it("sends auth token as X-OpenObscure-Token header", async () => {
    const { server, port } = await createAuthServer("my-secret-token");
    servers.push(server);

    const monitor = new HeartbeatMonitor({
      proxyUrl: `http://127.0.0.1:${port}`,
      intervalMs: 60_000,
      timeoutMs: 2_000,
      authToken: "my-secret-token",
    });
    monitor.start();

    const health = await monitor.check();
    assert.ok(health);
    assert.equal(health!.status, "ok");
    assert.equal(monitor.state, "active");

    monitor.stop();
  });

  it("missing auth token causes 401 and transitions to degraded", async () => {
    const { server, port } = await createAuthServer("required-token");
    servers.push(server);

    const monitor = new HeartbeatMonitor({
      proxyUrl: `http://127.0.0.1:${port}`,
      intervalMs: 60_000,
      timeoutMs: 2_000,
      // No authToken — should get 401
    });
    monitor.start();

    const health = await monitor.check();
    assert.equal(health, null);
    assert.equal(monitor.state, "degraded");

    monitor.stop();
  });

  it("preserves lastHealth after failure", async () => {
    const { server, port } = await createMockServer({
      pii_matches_total: 42,
    });
    servers.push(server);

    const monitor = new HeartbeatMonitor({
      proxyUrl: `http://127.0.0.1:${port}`,
      intervalMs: 60_000,
      timeoutMs: 2_000,
    });
    monitor.start();

    await monitor.check(); // Success
    assert.equal(monitor.lastHealth!.pii_matches_total, 42);

    // Close server to simulate failure
    server.close();
    servers = servers.filter((s) => s !== server);

    // Point to dead port
    (monitor as any).proxyUrl = "http://127.0.0.1:1";
    await monitor.check(); // Failure

    // lastHealth should still have the previous successful response
    assert.equal(monitor.lastHealth!.pii_matches_total, 42);

    monitor.stop();
  });
});

describe("STATE_MESSAGES", () => {
  it("has messages for degraded, recovering, disabled", () => {
    assert.ok(STATE_MESSAGES.degraded.includes("not responding"));
    assert.ok(STATE_MESSAGES.degraded.includes("cargo run"));
    assert.ok(STATE_MESSAGES.recovering.includes("recovered"));
    assert.ok(STATE_MESSAGES.disabled.includes("not enabled"));
  });

  it("active message is empty (silent)", () => {
    assert.equal(STATE_MESSAGES.active, "");
  });
});
