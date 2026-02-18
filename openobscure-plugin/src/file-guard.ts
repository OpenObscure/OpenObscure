/**
 * File Access Guard — controls which files/paths the agent can access.
 *
 * Prevents the agent from reading/writing sensitive files (credentials,
 * SSH keys, environment files, etc.). Registered as a tool that gates
 * file access decisions.
 */

import { ToolDefinition } from "./types";

/** Default deny list of sensitive file patterns. */
const DEFAULT_DENY_PATTERNS: RegExp[] = [
  // Credentials and secrets
  /\.env$/i,
  /\.env\.\w+$/i,
  /credentials\.json$/i,
  /\.credentials$/i,
  /secret[s]?\.ya?ml$/i,
  /secret[s]?\.json$/i,

  // SSH and GPG keys
  /\.ssh\/(?:id_|known_hosts|authorized_keys)/,
  /\.gnupg\//,

  // Cloud provider configs
  /\.aws\/credentials/,
  /\.aws\/config/,
  /\.azure\/accessTokens/,
  /\.config\/gcloud/,

  // Package manager tokens
  /\.npmrc$/,
  /\.pypirc$/,

  // Database files that may contain PII
  /\.sqlite3?$/i,
  /\.db$/i,

  // OS credential stores
  /keychain/i,
  /credential.store/i,

  // OpenObscure's own sensitive files
  /openobscure.*\.enc\.json$/,
];

export interface FileGuardConfig {
  /** Additional patterns to deny (merged with defaults). */
  extraDenyPatterns?: string[];
  /** Patterns to explicitly allow (overrides deny). */
  allowPatterns?: string[];
}

export interface FileCheckResult {
  allowed: boolean;
  reason?: string;
}

/** Check if a file path is allowed for agent access. */
export function checkFileAccess(
  filePath: string,
  config?: FileGuardConfig
): FileCheckResult {
  const normalized = filePath.replace(/\\/g, "/");

  // Check explicit allow list first (overrides deny)
  if (config?.allowPatterns) {
    for (const pattern of config.allowPatterns) {
      if (new RegExp(pattern).test(normalized)) {
        return { allowed: true };
      }
    }
  }

  // Check default deny patterns
  for (const pattern of DEFAULT_DENY_PATTERNS) {
    if (pattern.test(normalized)) {
      return {
        allowed: false,
        reason: `Path matches sensitive pattern: ${pattern.source}`,
      };
    }
  }

  // Check extra deny patterns from config
  if (config?.extraDenyPatterns) {
    for (const pattern of config.extraDenyPatterns) {
      if (new RegExp(pattern).test(normalized)) {
        return {
          allowed: false,
          reason: `Path matches custom deny pattern: ${pattern}`,
        };
      }
    }
  }

  return { allowed: true };
}

/** Create the file_access_check tool definition for registerTool. */
export function createFileGuardTool(
  config?: FileGuardConfig
): ToolDefinition {
  return {
    name: "openobscure_file_check",
    description:
      "Check if a file path is permitted for access. Call this before reading or writing sensitive files.",
    parameters: {
      path: {
        type: "string",
        description: "The file path to check",
        required: true,
      },
    },
    handler: async (params) => {
      const path = params.path as string;
      if (!path) {
        return JSON.stringify({ allowed: false, reason: "No path provided" });
      }
      const result = checkFileAccess(path, config);
      return JSON.stringify(result);
    },
  };
}
