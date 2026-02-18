/**
 * OpenObscure Core API — agent-agnostic privacy functions.
 *
 * Use this entry point when integrating with any AI agent framework.
 * For OpenClaw-specific integration (hooks + tool registration), use the
 * default entry point (index.ts) which exports the register() function.
 *
 * Usage:
 *   import { redactPii, checkFileAccess, ConsentManager } from "openobscure-plugin/core";
 */

// PII Redaction
export { redactPii } from "./redactor";
export type { RedactionResult } from "./redactor";

// File Access Guard
export { checkFileAccess } from "./file-guard";
export type { FileCheckResult, FileGuardConfig } from "./file-guard";

// GDPR Consent
export { ConsentManager, aiDisclosureText } from "./consent-manager";
export type {
  ConsentRecord,
  ConsentType,
  DsarRequest,
  DsarType,
  ProcessingLogEntry,
  PrivacyExport,
  ConsentStatus,
  RetentionTier,
  RetentionEntry,
  RetentionSummary,
} from "./consent-manager";

// Privacy Commands
export { handlePrivacyCommand } from "./privacy-commands";
export type { PrivacyCommandResult, PrivacyCommandOptions } from "./privacy-commands";

// Memory Governance
export { MemoryGovernor, DEFAULT_RETENTION_POLICY } from "./memory-governance";
export type { RetentionPolicy, EnforceResult } from "./memory-governance";

// Health Monitoring
export { HeartbeatMonitor, STATE_MESSAGES } from "./heartbeat";
export type { ProxyState, HealthResponse, HeartbeatConfig } from "./heartbeat";

// Logging
export {
  cgInfo, cgWarn, cgError, cgDebug, cgAudit,
  CG_MODULES, cgLogInit, cgLogShutdown,
} from "./cg-log";
export type { CgLogLevel, CgLogConfig } from "./cg-log";
