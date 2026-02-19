/**
 * OpenObscure Core API — agent-agnostic privacy functions.
 *
 * Use this entry point when integrating with any AI agent framework.
 * For OpenClaw-specific integration (hooks + tool registration), use the
 * default entry point (index.ts) which exports the register() function.
 *
 * Usage:
 *   import { redactPii, HeartbeatMonitor } from "openobscure-plugin/core";
 */

// PII Redaction
export { redactPii } from "./redactor";
export type { RedactionResult } from "./redactor";

// Health Monitoring
export { HeartbeatMonitor, STATE_MESSAGES } from "./heartbeat";
export type { ProxyState, HealthResponse, HeartbeatConfig } from "./heartbeat";

// Logging
export {
  ooInfo, ooWarn, ooError, ooDebug, ooAudit,
  OO_MODULES, ooLogInit, ooLogShutdown,
} from "./oo-log";
export type { OoLogLevel, OoLogConfig } from "./oo-log";
