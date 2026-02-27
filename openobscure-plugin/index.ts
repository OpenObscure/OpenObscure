/**
 * OpenObscure — OpenClaw Plugin Adapter
 *
 * Bridges the agent-agnostic core (src/index.ts) to OpenClaw's plugin API.
 * OpenClaw discovers this file via openclaw.plugin.json in the same directory.
 *
 * The core plugin uses a generic PluginAPI interface. This adapter translates
 * OpenClaw's hook system (api.on("tool_result_persist", ...)) into the core's
 * expected interface.
 */

import type { OpenObscurePluginConfig } from "./src/index.js";
import { redactPii, redactPiiWithNer } from "./src/redactor.js";
import { HeartbeatMonitor } from "./src/heartbeat.js";
import { ooInfo, OO_MODULES } from "./src/oo-log.js";
import * as fs from "fs";

const DEFAULT_CONFIG: Required<OpenObscurePluginConfig> = {
  redactToolResults: true,
  logStats: true,
  heartbeat: true,
  proxyUrl: "http://127.0.0.1:18790",
  heartbeatIntervalMs: 30_000,
};

function readAuthToken(): string | undefined {
  try {
    const home = process.env.HOME || process.env.USERPROFILE || ".";
    const tokenPath = `${home}/.openobscure/.auth-token`;
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

// OpenClaw plugin definition (default export)
const plugin = {
  id: "openobscure-plugin",
  name: "OpenObscure",
  description: "PII redaction and privacy controls for AI agents",

  register(api: any) {
    const cfg = { ...DEFAULT_CONFIG, ...(api.pluginConfig || {}) };
    const authToken = readAuthToken();
    const logger = api.logger || { info: () => {}, warn: () => {}, error: () => {} };

    // 1. Start heartbeat monitor
    let monitor: HeartbeatMonitor | undefined;
    if (cfg.heartbeat) {
      monitor = new HeartbeatMonitor({
        proxyUrl: cfg.proxyUrl,
        intervalMs: cfg.heartbeatIntervalMs,
        authToken,
      });
      monitor.start();
      logger.info(`Heartbeat monitor started (proxy: ${cfg.proxyUrl})`);
    }

    // 2. Register tool_result_persist hook for PII redaction
    if (cfg.redactToolResults && api.on) {
      api.on("tool_result_persist", (event: any, ctx: any) => {
        if (!event.message) return;

        // Extract text content from the agent message
        const content = typeof event.message.content === "string"
          ? event.message.content
          : Array.isArray(event.message.content)
            ? event.message.content
                .filter((b: any) => b.type === "text" || b.type === "tool_result")
                .map((b: any) => b.text || b.content || "")
                .join("\n")
            : "";

        if (!content) return;

        const useNer = monitor?.state === "active";
        const redacted = useNer
          ? redactPiiWithNer(content, cfg.proxyUrl, authToken)
          : redactPii(content);

        if (redacted.count > 0) {
          if (cfg.logStats) {
            const typeBreakdown = Object.entries(redacted.types)
              .map(([type, count]) => `${type}=${count}`)
              .join(", ");
            logger.info(`Redacted ${redacted.count} PII match(es) in tool result [${typeBreakdown}] (ner=${useNer})`);
          }

          // Return modified message
          if (typeof event.message.content === "string") {
            return { message: { ...event.message, content: redacted.text } };
          } else if (Array.isArray(event.message.content)) {
            // Rebuild content blocks with redacted text
            let redactedRemaining = redacted.text;
            const newContent = event.message.content.map((block: any) => {
              if (block.type === "text" || block.type === "tool_result") {
                const original = block.text || block.content || "";
                const piece = redactedRemaining.slice(0, original.length);
                redactedRemaining = redactedRemaining.slice(original.length + 1); // +1 for \n join
                return { ...block, text: piece, content: piece };
              }
              return block;
            });
            return { message: { ...event.message, content: newContent } };
          }
        }
      });
    }

    // 3. Register before_tool_call hook (hard enforcement)
    if (cfg.redactToolResults && api.on) {
      api.on("before_tool_call", (event: any, ctx: any) => {
        const argsText = JSON.stringify(event.params || {});
        const useNer = monitor?.state === "active";
        const redacted = useNer
          ? redactPiiWithNer(argsText, cfg.proxyUrl, authToken)
          : redactPii(argsText);

        if (redacted.count > 0) {
          logger.info(`Redacted ${redacted.count} PII match(es) in tool call args (${event.toolName})`);
          try {
            return { params: JSON.parse(redacted.text) };
          } catch {
            // If redacted text isn't valid JSON, don't modify
          }
        }
      });
    }

    logger.info("OpenObscure plugin registered (redactor=" + cfg.redactToolResults + ", heartbeat=" + cfg.heartbeat + ")");
  },
};

export default plugin;
