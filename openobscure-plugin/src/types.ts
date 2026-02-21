/**
 * Agent Plugin Adapter types — generic interface for AI agent integration.
 *
 * Implements a hook-based plugin architecture (used by OpenClaw and compatible agents):
 * - Plugins export a register(api) function
 * - tool_result_persist hook is synchronous (Promise causes silent skip in some frameworks)
 * - registerTool injects tools into the agent's decision flow
 */

/** Tool result before transcript persistence. */
export interface ToolResult {
  /** Tool name that produced this result. */
  tool_name: string;
  /** The result content (may contain PII). */
  content: string;
  /** Whether this result was an error. */
  is_error?: boolean;
  /** Optional metadata. */
  metadata?: Record<string, unknown>;
}

/** Tool definition for registerTool. */
export interface ToolDefinition {
  name: string;
  description: string;
  parameters?: Record<string, ParameterDef>;
  handler: (params: Record<string, unknown>) => Promise<string>;
}

export interface ParameterDef {
  type: string;
  description: string;
  required?: boolean;
}

/** Tool call before execution (for pre-execution interception). */
export interface ToolCall {
  /** Tool name to be called. */
  tool_name: string;
  /** Tool arguments (may contain PII). */
  arguments: Record<string, unknown>;
  /** Optional metadata. */
  metadata?: Record<string, unknown>;
}

/** The plugin API provided to register(). */
export interface PluginAPI {
  hooks: {
    /** Modify tool results synchronously before transcript persistence. */
    tool_result_persist: (handler: (result: ToolResult) => ToolResult) => void;
    /**
     * Intercept tool calls before execution (hard enforcement).
     * NOT YET WIRED in OpenClaw — defined in types but never invoked.
     * When this hook becomes available, it enables pre-execution PII
     * redaction: tool arguments are sanitized BEFORE the tool runs.
     */
    before_tool_call?: (handler: (call: ToolCall) => ToolCall | null) => void;
  };
  /** Register a custom tool the agent can call. */
  registerTool: (tool: ToolDefinition) => void;
}
