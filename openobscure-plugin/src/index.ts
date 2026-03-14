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

import { PluginAPI, ToolCall, ToolResult } from "./types";
import { redactPii, redactPiiWithNer, activeEngine } from "./redactor";
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

  // Read auth token early (needed by both NER and heartbeat)
  const authToken = readAuthToken();

  // Log which scanner engine loaded (napi = full 14-type, js = 5-type fallback)
  ooInfo(OO_MODULES.PLUGIN, "Scanner engine active", { engine: activeEngine() });

  // 1. Start L1 heartbeat monitor for L0 proxy health
  //    Must start before registering the hook so state is available.
  let monitor: HeartbeatMonitor | undefined;
  if (cfg.heartbeat) {
    monitor = new HeartbeatMonitor({
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

  // 2. Register tool_result_persist hook for PII redaction
  if (cfg.redactToolResults) {
    const proxyUrl = cfg.proxyUrl;
    const logStats = cfg.logStats;

    api.hooks.tool_result_persist((result: ToolResult): ToolResult => {
      // MUST be synchronous — Promise causes silent skip in OpenClaw
      // Use NER-enhanced redaction when L0 is healthy, regex-only otherwise
      const useNer = monitor?.state === "active";
      const redacted = useNer
        ? redactPiiWithNer(result.content, proxyUrl, authToken)
        : redactPii(result.content);

      if (redacted.count > 0 && logStats) {
        // Log stats without logging actual PII values
        const typeBreakdown = Object.entries(redacted.types)
          .map(([type, count]) => `${type}=${count}`)
          .join(", ");
        ooInfo(OO_MODULES.REDACTOR, "Redacted PII in tool result", {
          count: redacted.count,
          tool: result.tool_name,
          types: typeBreakdown,
          ner: useNer,
        });
      }

      return {
        ...result,
        content: redacted.text,
      };
    });
  }

  // 3. Register before_tool_call hook for pre-execution PII enforcement.
  //    This provides hard enforcement: tool arguments are sanitised BEFORE the
  //    tool runs, so PII never reaches file-write, HTTP, or shell tool calls.
  //    The hook is defined in PluginAPI but not yet wired in the host agent;
  //    the `if (api.hooks.before_tool_call)` guard makes this a no-op until wired.
  if (cfg.redactToolResults && api.hooks.before_tool_call) {
    try {
      const proxyUrlBtc = cfg.proxyUrl;
      api.hooks.before_tool_call((call: ToolCall): ToolCall | null => {
        // Serialize arguments to text for PII scanning
        const argsText = JSON.stringify(call.arguments);
        const useNer = monitor?.state === "active";
        const redacted = useNer
          ? redactPiiWithNer(argsText, proxyUrlBtc, authToken)
          : redactPii(argsText);

        if (redacted.count > 0) {
          ooInfo(OO_MODULES.REDACTOR, "Redacted PII in tool call (hard enforcement)", {
            count: redacted.count,
            tool: call.tool_name,
          });
          return {
            ...call,
            arguments: JSON.parse(redacted.text),
          };
        }
        return call;
      });
      ooInfo(OO_MODULES.PLUGIN, "before_tool_call hook registered — hard enforcement active");
    } catch {
      // Hook not wired yet — fall back to tool_result_persist only
      ooInfo(OO_MODULES.PLUGIN, "before_tool_call hook not available — soft enforcement only");
    }
  }

  ooInfo(OO_MODULES.PLUGIN, "Plugin registered", {
    redactor: cfg.redactToolResults,
    heartbeat: cfg.heartbeat,
  });
}

// Re-export for direct use
export { redactPii, redactPiiWithNer, activeEngine } from "./redactor";
export type { ScannerEngine } from "./redactor";
export type { PluginAPI, ToolCall, ToolResult, ToolDefinition } from "./types";
export type { RedactionResult, NerMatch } from "./redactor";
export { HeartbeatMonitor, STATE_MESSAGES } from "./heartbeat";
export type {
  ProxyState,
  HealthResponse,
  HeartbeatConfig,
} from "./heartbeat";
