import { describe, it, beforeEach } from "node:test";
import assert from "node:assert/strict";
import { register } from "./index";
import type { PluginAPI, ToolCall, ToolResult } from "./types";

/**
 * Tests for the before_tool_call prepared handler.
 *
 * Verifies that register() correctly:
 * 1. Registers the before_tool_call hook when available
 * 2. Falls back gracefully when hook is not available
 * 3. Redacts PII in tool call arguments (hard enforcement)
 * 4. Passes clean arguments through unchanged
 */

// Mock PluginAPI with before_tool_call support
function createMockApi(opts?: { supportBeforeToolCall?: boolean }): {
  api: PluginAPI;
  capturedToolResultHandler: ((result: ToolResult) => ToolResult) | null;
  capturedBeforeToolCallHandler: ((call: ToolCall) => ToolCall | null) | null;
} {
  let capturedToolResultHandler: ((result: ToolResult) => ToolResult) | null = null;
  let capturedBeforeToolCallHandler: ((call: ToolCall) => ToolCall | null) | null = null;

  const hooks: PluginAPI["hooks"] = {
    tool_result_persist: (handler) => {
      capturedToolResultHandler = handler;
    },
  };

  if (opts?.supportBeforeToolCall) {
    hooks.before_tool_call = (handler) => {
      capturedBeforeToolCallHandler = handler;
    };
  }

  const api: PluginAPI = {
    hooks,
    registerTool: () => {},
  };

  return { api, capturedToolResultHandler: null, capturedBeforeToolCallHandler: null, get _trh() { return capturedToolResultHandler; }, get _btch() { return capturedBeforeToolCallHandler; } };
}

describe("before_tool_call handler", () => {
  it("registers before_tool_call when hook is available", () => {
    let capturedHandler: ((call: ToolCall) => ToolCall | null) | null = null;

    const api: PluginAPI = {
      hooks: {
        tool_result_persist: () => {},
        before_tool_call: (handler) => {
          capturedHandler = handler;
        },
      },
      registerTool: () => {},
    };

    register(api, { heartbeat: false });
    assert.notEqual(capturedHandler, null, "before_tool_call handler should be registered");
  });

  it("does not throw when before_tool_call hook is not available", () => {
    const api: PluginAPI = {
      hooks: {
        tool_result_persist: () => {},
        // before_tool_call intentionally missing
      },
      registerTool: () => {},
    };

    // Should not throw
    assert.doesNotThrow(() => {
      register(api, { heartbeat: false });
    });
  });

  it("redacts PII in tool call arguments", () => {
    let capturedHandler: ((call: ToolCall) => ToolCall | null) | null = null;

    const api: PluginAPI = {
      hooks: {
        tool_result_persist: () => {},
        before_tool_call: (handler) => {
          capturedHandler = handler;
        },
      },
      registerTool: () => {},
    };

    register(api, { heartbeat: false });
    assert.notEqual(capturedHandler, null);

    const call: ToolCall = {
      tool_name: "send_email",
      arguments: {
        to: "test@example.com",
        body: "My SSN is 123-45-6789",
      },
    };

    const result = capturedHandler!(call);
    assert.notEqual(result, null);
    assert.equal(result!.tool_name, "send_email");
    // SSN should be redacted in the body
    assert.ok(
      !(result!.arguments as Record<string, string>).body.includes("123-45-6789"),
      "SSN should be redacted in tool arguments"
    );
    assert.ok(
      (result!.arguments as Record<string, string>).body.includes("[REDACTED-SSN]"),
      "SSN should be replaced with [REDACTED-SSN]"
    );
  });

  it("passes clean arguments through unchanged", () => {
    let capturedHandler: ((call: ToolCall) => ToolCall | null) | null = null;

    const api: PluginAPI = {
      hooks: {
        tool_result_persist: () => {},
        before_tool_call: (handler) => {
          capturedHandler = handler;
        },
      },
      registerTool: () => {},
    };

    register(api, { heartbeat: false });
    assert.notEqual(capturedHandler, null);

    const call: ToolCall = {
      tool_name: "read_file",
      arguments: {
        path: "/home/user/document.txt",
      },
    };

    const result = capturedHandler!(call);
    assert.notEqual(result, null);
    assert.equal(result!.tool_name, "read_file");
    assert.deepEqual(result!.arguments, { path: "/home/user/document.txt" });
  });

  it("redacts credit card in nested arguments", () => {
    let capturedHandler: ((call: ToolCall) => ToolCall | null) | null = null;

    const api: PluginAPI = {
      hooks: {
        tool_result_persist: () => {},
        before_tool_call: (handler) => {
          capturedHandler = handler;
        },
      },
      registerTool: () => {},
    };

    register(api, { heartbeat: false });
    assert.notEqual(capturedHandler, null);

    const call: ToolCall = {
      tool_name: "write_file",
      arguments: {
        path: "/tmp/data.json",
        content: "Payment card: 4111111111111111",
      },
    };

    const result = capturedHandler!(call);
    assert.notEqual(result, null);
    assert.ok(
      !(result!.arguments as Record<string, string>).content.includes("4111111111111111"),
      "Credit card should be redacted"
    );
  });

  it("still registers tool_result_persist alongside before_tool_call", () => {
    let toolResultRegistered = false;
    let beforeToolCallRegistered = false;

    const api: PluginAPI = {
      hooks: {
        tool_result_persist: () => {
          toolResultRegistered = true;
        },
        before_tool_call: () => {
          beforeToolCallRegistered = true;
        },
      },
      registerTool: () => {},
    };

    register(api, { heartbeat: false });
    assert.ok(toolResultRegistered, "tool_result_persist should always be registered");
    assert.ok(beforeToolCallRegistered, "before_tool_call should be registered when available");
  });

  it("does not register before_tool_call when redactToolResults is false", () => {
    let beforeToolCallRegistered = false;

    const api: PluginAPI = {
      hooks: {
        tool_result_persist: () => {},
        before_tool_call: () => {
          beforeToolCallRegistered = true;
        },
      },
      registerTool: () => {},
    };

    register(api, { heartbeat: false, redactToolResults: false });
    assert.ok(!beforeToolCallRegistered, "before_tool_call should not register when redaction is disabled");
  });
});
