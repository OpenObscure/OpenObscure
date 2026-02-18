/**
 * OpenObscure Gateway Plugin — Entry Point
 *
 * Registers with the host agent's plugin API (e.g., OpenClaw) to provide:
 * 1. PII Redaction via tool_result_persist hook (synchronous)
 * 2. File Access Guard via registerTool
 * 3. GDPR Consent Manager with /privacy commands
 * 4. L1 Heartbeat Monitor — detects L0 proxy outages
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
import { checkFileAccess, createFileGuardTool, FileGuardConfig } from "./file-guard";
import { ConsentManager } from "./consent-manager";
import { handlePrivacyCommand } from "./privacy-commands";
import { MemoryGovernor } from "./memory-governance";
import * as fs from "fs";
import { HeartbeatMonitor } from "./heartbeat";
import { ooInfo, OO_MODULES } from "./oo-log";

export interface OpenObscurePluginConfig {
  /** Enable PII redaction in tool results (default: true). */
  redactToolResults?: boolean;
  /** Enable file access guard tool (default: true). */
  fileGuard?: boolean;
  /** File guard configuration. */
  fileGuardConfig?: FileGuardConfig;
  /** Log redaction statistics (default: true). */
  logStats?: boolean;
  /** Enable GDPR consent manager (default: true). */
  consentManager?: boolean;
  /** Path to consent SQLite database (default: ~/.openobscure/consent.db). */
  consentDbPath?: string;
  /** Directory for privacy data exports (default: ~/.openobscure/exports). */
  exportDir?: string;
  /** Default user ID when not provided by context (default: "default"). */
  defaultUserId?: string;
  /** Enable L1 heartbeat monitor for L0 proxy health (default: true). */
  heartbeat?: boolean;
  /** L0 proxy base URL for heartbeat (default: http://127.0.0.1:18790). */
  proxyUrl?: string;
  /** Heartbeat interval in milliseconds (default: 30000). */
  heartbeatIntervalMs?: number;
  /** Enable memory governance with retention tiers (default: true when consentManager is true). */
  memoryGovernance?: boolean;
  /** Retention enforcement interval in milliseconds (default: 3600000 = 1 hour). */
  retentionIntervalMs?: number;
}

const DEFAULT_CONFIG: Required<OpenObscurePluginConfig> = {
  redactToolResults: true,
  fileGuard: true,
  fileGuardConfig: {},
  logStats: true,
  consentManager: true,
  consentDbPath: "",
  exportDir: "",
  defaultUserId: "default",
  heartbeat: true,
  proxyUrl: "http://127.0.0.1:18790",
  heartbeatIntervalMs: 30_000,
  memoryGovernance: true,
  retentionIntervalMs: 3_600_000, // 1 hour
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

  // 2. Register file access guard tool
  if (cfg.fileGuard) {
    api.registerTool(createFileGuardTool(cfg.fileGuardConfig));
  }

  // 3. Register GDPR consent manager and /privacy command tool
  if (cfg.consentManager) {
    const dbPath = cfg.consentDbPath || resolveDefaultPath("consent.db");
    const exportDir = cfg.exportDir || resolveDefaultPath("exports");

    const consent = new ConsentManager(dbPath);

    // Initialize memory governor if enabled
    const governor = cfg.memoryGovernance
      ? new MemoryGovernor(consent)
      : undefined;

    api.registerTool({
      name: "openobscure_privacy",
      description:
        "Manage privacy settings, consent, and data access requests. " +
        "Subcommands: status, consent grant/revoke, export, delete, disclosure, retention status/enforce/policy.",
      parameters: {
        args: {
          type: "string",
          description:
            'Space-separated command arguments (e.g., "status", "consent grant processing", "export", "delete", "disclosure Claude Anthropic", "retention status")',
          required: true,
        },
        user_id: {
          type: "string",
          description: "User ID (optional, defaults to configured default)",
        },
      },
      handler: async (params: Record<string, unknown>): Promise<string> => {
        const argsStr = (params.args as string) || "";
        const userId = (params.user_id as string) || cfg.defaultUserId;
        const args = argsStr.split(/\s+/).filter(Boolean);
        const result = handlePrivacyCommand(consent, userId, args, {
          exportDir,
          governor,
        });
        return result.text;
      },
    });

    // 3b. Start periodic retention enforcement
    if (governor) {
      setInterval(() => {
        const result = governor.enforce();
        if (result.promoted > 0 || result.pruned > 0) {
          ooInfo(OO_MODULES.CONSENT, "Retention enforcement ran", {
            promoted: result.promoted,
            pruned: result.pruned,
          });
        }
      }, cfg.retentionIntervalMs);
      ooInfo(OO_MODULES.CONSENT, "Memory governance enabled", {
        interval: cfg.retentionIntervalMs,
      });
    }

    ooInfo(OO_MODULES.CONSENT, "Consent manager initialized", { db: dbPath });
  }

  // 4. Start L1 heartbeat monitor for L0 proxy health
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
    fileGuard: cfg.fileGuard,
    consent: cfg.consentManager,
    heartbeat: cfg.heartbeat,
  });
}

// Re-export for direct use
export { redactPii } from "./redactor";
export { checkFileAccess, createFileGuardTool } from "./file-guard";
export { ConsentManager, aiDisclosureText } from "./consent-manager";
export { handlePrivacyCommand } from "./privacy-commands";
export type { PluginAPI, ToolResult, ToolDefinition } from "./types";
export type { RedactionResult } from "./redactor";
export type { FileCheckResult, FileGuardConfig } from "./file-guard";
export type {
  ConsentRecord,
  ConsentType,
  DsarRequest,
  DsarType,
  ProcessingLogEntry,
  PrivacyExport,
  ConsentStatus,
} from "./consent-manager";
export type { PrivacyCommandResult, PrivacyCommandOptions } from "./privacy-commands";
export { MemoryGovernor, DEFAULT_RETENTION_POLICY } from "./memory-governance";
export type { RetentionPolicy, EnforceResult } from "./memory-governance";
export type { RetentionTier, RetentionEntry, RetentionSummary } from "./consent-manager";
export { HeartbeatMonitor, STATE_MESSAGES } from "./heartbeat";
export type {
  ProxyState,
  HealthResponse,
  HeartbeatConfig,
} from "./heartbeat";
