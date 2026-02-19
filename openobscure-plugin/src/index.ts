/**
 * OpenObscure Gateway Plugin — Entry Point
 *
 * Registers with the host agent's plugin API (e.g., OpenClaw) to provide:
 * 1. PII Redaction via tool_result_persist hook (synchronous)
 * 2. L1 Heartbeat Monitor — detects L0 proxy outages
 *
 * This is L1 (second line of defense). L0 (Rust proxy) handles
 * FPE encryption before data reaches the LLM. L1 catches PII in
 * tool results that bypass the proxy.
 *
 * For agent-agnostic core functions (without hook/tool wiring),
 * use the "openobscure-plugin/core" entry point instead.
 */

import { PluginAPI, ToolResult } from "./types";
import { redactPii } from "./redactor";
import * as fs from "fs";
import { HeartbeatMonitor } from "./heartbeat";
import { ooInfo, OO_MODULES } from "./oo-log";

export interface OpenObscurePluginConfig {
  /** Enable PII redaction in tool results (default: true). */
  redactToolResults?: boolean;
  /** Log redaction statistics (default: true). */
  logStats?: boolean;
  /** Enable L1 heartbeat monitor for L0 proxy health (default: true). */
  heartbeat?: boolean;
  /** L0 proxy base URL for heartbeat (default: http://127.0.0.1:18790). */
  proxyUrl?: string;
  /** Heartbeat interval in milliseconds (default: 30000). */
  heartbeatIntervalMs?: number;
}

const DEFAULT_CONFIG: Required<OpenObscurePluginConfig> = {
  redactToolResults: true,
  logStats: true,
  heartbeat: true,
  proxyUrl: "http://127.0.0.1:18790",
  heartbeatIntervalMs: 30_000,
};

function resolveDefaultPath(subpath: string): string {
  const home = process.env.HOME || process.env.USERPROFILE || ".";
  return `${home}/.openobscure/${subpath}`;
}

/** Read L0 auth token from ~/.openobscure/.auth-token (written by L0 on startup). */
function readAuthToken(): string | undefined {
  try {
    const tokenPath = resolveDefaultPath(".auth-token");
    const token = fs.readFileSync(tokenPath, "utf-8").trim();
    if (token) {
      ooInfo(OO_MODULES.PLUGIN, "Auth token loaded", { path: tokenPath });
      return token;
    }
  } catch {
    // Token file doesn't exist yet — L0 may not have started
  }
  return undefined;
}

/**
 * OpenClaw plugin entry point.
 * Called by the Gateway when the plugin is loaded.
 */
export function register(api: PluginAPI, config?: OpenObscurePluginConfig): void {
  const cfg = { ...DEFAULT_CONFIG, ...config };

  // 1. Register tool_result_persist hook for PII redaction
  if (cfg.redactToolResults) {
    api.hooks.tool_result_persist((result: ToolResult): ToolResult => {
      // MUST be synchronous — Promise causes silent skip in OpenClaw
      const redacted = redactPii(result.content);

      if (redacted.count > 0 && cfg.logStats) {
        // Log stats without logging actual PII values
        const typeBreakdown = Object.entries(redacted.types)
          .map(([type, count]) => `${type}=${count}`)
          .join(", ");
        ooInfo(OO_MODULES.REDACTOR, "Redacted PII in tool result", {
          count: redacted.count,
          tool: result.tool_name,
          types: typeBreakdown,
        });
      }

      return {
        ...result,
        content: redacted.text,
      };
    });
  }

  // 2. Start L1 heartbeat monitor for L0 proxy health
  if (cfg.heartbeat) {
    const authToken = readAuthToken();
    const monitor = new HeartbeatMonitor({
      proxyUrl: cfg.proxyUrl,
      intervalMs: cfg.heartbeatIntervalMs,
      authToken,
    });
    monitor.start();
    ooInfo(OO_MODULES.HEARTBEAT, "Heartbeat monitor started", {
      proxy: cfg.proxyUrl,
      interval: cfg.heartbeatIntervalMs,
    });
  }

  ooInfo(OO_MODULES.PLUGIN, "Plugin registered", {
    redactor: cfg.redactToolResults,
    heartbeat: cfg.heartbeat,
  });
}

// Re-export for direct use
export { redactPii } from "./redactor";
export type { PluginAPI, ToolResult, ToolDefinition } from "./types";
export type { RedactionResult } from "./redactor";
export { HeartbeatMonitor, STATE_MESSAGES } from "./heartbeat";
export type {
  ProxyState,
  HealthResponse,
  HeartbeatConfig,
} from "./heartbeat";
