/**
 * L1 Heartbeat Monitor — pings L0 proxy health endpoint to detect outages.
 *
 * States:
 * - active:     L0 is responding (silent — no user notification)
 * - degraded:   L0 stopped responding (warn user)
 * - recovering: L0 just came back after being down (log recovery)
 * - disabled:   Monitor not started / explicitly stopped
 */

import * as http from "http";
import { cgInfo, cgWarn, CG_MODULES } from "./cg-log";

export type ProxyState = "active" | "degraded" | "recovering" | "disabled";

export interface HealthResponse {
  status: string;
  version: string;
  uptime_secs: number;
  pii_matches_total: number;
  requests_total: number;
}

export interface HeartbeatConfig {
  /** L0 proxy base URL (default: http://127.0.0.1:18790). */
  proxyUrl?: string;
  /** Heartbeat interval in milliseconds (default: 30000 = 30s). */
  intervalMs?: number;
  /** HTTP request timeout in milliseconds (default: 5000 = 5s). */
  timeoutMs?: number;
  /** Auth token for L0 health endpoint (sent as X-OpenObscure-Token header). */
  authToken?: string;
  /** Callback when proxy state changes. */
  onStateChange?: (state: ProxyState, message: string) => void;
}

const DEFAULT_HEARTBEAT_CONFIG: Required<Omit<HeartbeatConfig, "onStateChange" | "authToken">> = {
  proxyUrl: "http://127.0.0.1:18790",
  intervalMs: 30_000,
  timeoutMs: 5_000,
};

/** User-facing messages for each state transition. */
export const STATE_MESSAGES: Record<ProxyState, string> = {
  active: "", // Silent when working
  degraded:
    "OpenObscure proxy is not responding — PII protection is disabled",
  recovering: "OpenObscure proxy recovered",
  disabled:
    "OpenObscure is not enabled. PII will be sent in plaintext.",
};

export class HeartbeatMonitor {
  private proxyUrl: string;
  private intervalMs: number;
  private timeoutMs: number;
  private onStateChange: (state: ProxyState, message: string) => void;
  private authToken: string | undefined;

  private timer: ReturnType<typeof setInterval> | null = null;
  private _state: ProxyState = "disabled";
  private _lastHealth: HealthResponse | null = null;
  private _consecutiveFailures: number = 0;

  constructor(config?: HeartbeatConfig) {
    this.proxyUrl = config?.proxyUrl ?? DEFAULT_HEARTBEAT_CONFIG.proxyUrl;
    this.intervalMs =
      config?.intervalMs ?? DEFAULT_HEARTBEAT_CONFIG.intervalMs;
    this.timeoutMs =
      config?.timeoutMs ?? DEFAULT_HEARTBEAT_CONFIG.timeoutMs;
    this.authToken = config?.authToken;
    this.onStateChange =
      config?.onStateChange ?? defaultStateChangeHandler;
  }

  /** Current proxy state. */
  get state(): ProxyState {
    return this._state;
  }

  /** Last successful health response (null if never received). */
  get lastHealth(): HealthResponse | null {
    return this._lastHealth;
  }

  /** Number of consecutive health check failures. */
  get consecutiveFailures(): number {
    return this._consecutiveFailures;
  }

  /** Start the heartbeat monitor. Performs an immediate check, then repeats. */
  start(): void {
    if (this.timer) return; // Already running

    this._state = "active"; // Assume active until proven otherwise
    // First check runs after one interval; callers who want an immediate
    // check can await monitor.check() explicitly.
    this.timer = setInterval(() => this.tick(), this.intervalMs);
  }

  /** Stop the heartbeat monitor. */
  stop(): void {
    if (this.timer) {
      clearInterval(this.timer);
      this.timer = null;
    }
    this.transition("disabled");
  }

  /** Perform a single health check (exposed for testing). */
  async check(): Promise<HealthResponse | null> {
    try {
      const response = await this.fetchHealth();
      this._lastHealth = response;
      this._consecutiveFailures = 0;

      if (this._state === "degraded") {
        this.transition("recovering");
        // Quickly transition back to active
        this.transition("active");
      } else if (this._state !== "active") {
        this.transition("active");
      }

      return response;
    } catch {
      this._consecutiveFailures++;
      if (this._state === "active" || this._state === "recovering") {
        this.transition("degraded");
      }
      return null;
    }
  }

  private tick(): void {
    // Fire-and-forget — don't block the interval
    this.check().catch(() => {});
  }

  private transition(newState: ProxyState): void {
    if (newState === this._state) return;
    this._state = newState;
    const message = STATE_MESSAGES[newState];
    if (message) {
      this.onStateChange(newState, message);
    }
  }

  private fetchHealth(): Promise<HealthResponse> {
    const url = `${this.proxyUrl}/_openobscure/health`;
    const headers: Record<string, string> = {};
    if (this.authToken) {
      headers["x-openobscure-token"] = this.authToken;
    }

    return new Promise((resolve, reject) => {
      const req = http.get(url, { timeout: this.timeoutMs, headers }, (res) => {
        if (res.statusCode !== 200) {
          reject(new Error(`Health check returned ${res.statusCode}`));
          res.resume(); // Drain response
          return;
        }

        let data = "";
        res.on("data", (chunk: Buffer) => {
          data += chunk.toString();
        });
        res.on("end", () => {
          try {
            const parsed = JSON.parse(data) as HealthResponse;
            resolve(parsed);
          } catch (e) {
            reject(new Error("Invalid health response JSON"));
          }
        });
      });

      req.on("error", (e: Error) => reject(e));
      req.on("timeout", () => {
        req.destroy();
        reject(new Error("Health check timed out"));
      });
    });
  }
}

function defaultStateChangeHandler(
  state: ProxyState,
  message: string
): void {
  switch (state) {
    case "degraded":
      cgWarn(CG_MODULES.HEARTBEAT, message);
      break;
    case "recovering":
      cgInfo(CG_MODULES.HEARTBEAT, message);
      break;
    case "disabled":
      cgWarn(CG_MODULES.HEARTBEAT, message);
      break;
    default:
      break; // active = silent
  }
}
