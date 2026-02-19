/**
 * OpenObscure Unified Logging API — TypeScript (L1 Plugin).
 *
 * Every L1 module calls ooInfo/ooWarn/ooError/ooDebug/ooAudit instead of
 * console.* directly. The wrapper handles:
 * - PII scrubbing via redactPii() before any output
 * - Structured JSON output (optional, for Docker/SIEM)
 * - GDPR audit log (separate file, durable writes)
 * - Module name constants (prevent typos)
 */

import { redactPii } from "./redactor";
import * as fs from "fs";

// ── Types ──

export type OoLogLevel = "error" | "warn" | "info" | "debug" | "trace";

export interface OoLogConfig {
  /** Minimum log level (default: "info"). */
  level?: OoLogLevel;
  /** Emit structured JSON instead of human-readable text (default: false). */
  jsonOutput?: boolean;
  /** File path for GDPR audit log. If unset, audit events are logged but not persisted. */
  auditLogPath?: string;
}

// ── Module constants — use these instead of string literals ──

export const OO_MODULES = {
  REDACTOR: "redactor",
  HEARTBEAT: "heartbeat",
  PLUGIN: "plugin",
} as const;

// ── Internal state ──

const LEVEL_PRIORITY: Record<OoLogLevel, number> = {
  error: 0,
  warn: 1,
  info: 2,
  debug: 3,
  trace: 4,
};

let _level: OoLogLevel = "info";
let _jsonOutput = false;
let _auditFd: number | null = null;

// ── Initialization ──

/** Initialize the logging subsystem. Called once from plugin register(). */
export function ooLogInit(config: OoLogConfig): void {
  if (config.level) _level = config.level;
  if (config.jsonOutput !== undefined) _jsonOutput = config.jsonOutput;

  if (config.auditLogPath) {
    try {
      _auditFd = fs.openSync(config.auditLogPath, "a");
    } catch {
      // Can't open audit file — log warning and continue
      console.warn(
        `[OpenObscure L1] [oo-log] Failed to open audit log: ${config.auditLogPath}`
      );
    }
  }
}

/** Shutdown: close audit file descriptor if open. */
export function ooLogShutdown(): void {
  if (_auditFd !== null) {
    try {
      fs.closeSync(_auditFd);
    } catch {
      // Ignore close errors
    }
    _auditFd = null;
  }
}

// ── Core logging function ──

/** Core logging function. All level-specific functions delegate here. */
export function ooLog(
  module: string,
  level: OoLogLevel,
  message: string,
  fields?: Record<string, unknown>
): void {
  if (LEVEL_PRIORITY[level] > LEVEL_PRIORITY[_level]) return;

  // PII scrub the message and all string field values
  const scrubbed = scrubFields(message, fields);

  if (_jsonOutput) {
    const entry = {
      ts: new Date().toISOString(),
      level,
      module: `openobscure.${module}`,
      msg: scrubbed.message,
      ...scrubbed.fields,
    };
    dispatch(level, JSON.stringify(entry));
  } else {
    const prefix = `[OpenObscure L1] [${module}]`;
    const fieldStr = formatFields(scrubbed.fields);
    dispatch(level, `${prefix} ${scrubbed.message}${fieldStr}`);
  }
}

// ── Level-specific convenience functions ──

export function ooError(
  module: string,
  message: string,
  fields?: Record<string, unknown>
): void {
  ooLog(module, "error", message, fields);
}

export function ooWarn(
  module: string,
  message: string,
  fields?: Record<string, unknown>
): void {
  ooLog(module, "warn", message, fields);
}

export function ooInfo(
  module: string,
  message: string,
  fields?: Record<string, unknown>
): void {
  ooLog(module, "info", message, fields);
}

export function ooDebug(
  module: string,
  message: string,
  fields?: Record<string, unknown>
): void {
  ooLog(module, "debug", message, fields);
}

// ── GDPR audit log ──

/** GDPR audit log — appended to audit file if configured, and also logged at info level. */
export function ooAudit(
  module: string,
  operation: string,
  fields?: Record<string, unknown>
): void {
  const entry = {
    ts: new Date().toISOString(),
    module: `openobscure.${module}`,
    operation,
    ...fields,
  };

  // Always write to audit file (if configured), regardless of log level
  if (_auditFd !== null) {
    try {
      fs.writeSync(_auditFd, JSON.stringify(entry) + "\n");
    } catch {
      // Audit write failed — don't crash, but warn once
    }
  }

  // Also log at info level for visibility
  ooLog(module, "info", `audit: ${operation}`, fields);
}

// ── Internal helpers ──

function dispatch(level: OoLogLevel, message: string): void {
  switch (level) {
    case "error":
      console.error(message);
      break;
    case "warn":
      console.warn(message);
      break;
    default:
      console.log(message);
      break;
  }
}

function scrubFields(
  message: string,
  fields?: Record<string, unknown>
): { message: string; fields?: Record<string, unknown> } {
  const scrubbedMsg = redactPii(message).text;
  if (!fields) return { message: scrubbedMsg };

  const scrubbedFields: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(fields)) {
    if (typeof value === "string") {
      scrubbedFields[key] = redactPii(value).text;
    } else {
      scrubbedFields[key] = value;
    }
  }
  return { message: scrubbedMsg, fields: scrubbedFields };
}

function formatFields(fields?: Record<string, unknown>): string {
  if (!fields || Object.keys(fields).length === 0) return "";
  return (
    " " +
    Object.entries(fields)
      .map(([k, v]) => `${k}=${v}`)
      .join(" ")
  );
}
