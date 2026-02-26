#!/usr/bin/env node
// ri_mock_server.mjs — Mock upstream server for Response Integrity testing.
//
// Returns canned Anthropic API responses based on the X-Mock-Response header:
//   "clean"       — benign response (no persuasion phrases)
//   "persuasive"  — urgency + scarcity + authority phrases
//   "commercial"  — commercial pressure + urgency (triggers Warning)
//   "fear"        — fear-based + commercial (triggers Caution)
//   "echo"        — echoes the user message content back as the response
//
// If no header is provided, returns "clean" by default.
//
// Usage:
//   node test/scripts/mock/ri_mock_server.mjs
//
// Environment:
//   RI_MOCK_PORT — Listen port (default: 18793)

import { createServer } from "http";

const PORT = parseInt(process.env.RI_MOCK_PORT || "18793");

const RESPONSES = {
  clean: {
    text: "The weather today is sunny with a high of 72 degrees. Remember to stay hydrated and wear sunscreen if spending time outdoors.",
  },
  persuasive: {
    text: "Act now before this limited time offer expires! Experts agree this is the smart choice. Don't miss out on this incredible opportunity that thousands of satisfied customers have already taken advantage of.",
  },
  commercial: {
    text: "Buy now and save 50%! This limited time offer won't last. Smart shoppers know that acting quickly is the key to getting the best deals. You deserve this premium product.",
  },
  fear: {
    text: "Act now or you could lose everything! Experts agree the risks are severe. This limited time protection plan is your only defense. Buy now before it's too late — smart choice for anyone who values their security.",
  },
};

let requestCount = 0;

const server = createServer((req, res) => {
  let body = "";
  req.on("data", (chunk) => (body += chunk));
  req.on("end", () => {
    requestCount++;

    const mockType = req.headers["x-mock-response"] || "clean";

    // Echo mode: return the user's message content as the response
    let responseText;
    if (mockType === "echo") {
      try {
        const parsed = JSON.parse(body);
        const msgs = parsed.messages || [];
        const userMsg = msgs.find((m) => m.role === "user");
        responseText = userMsg?.content || "No user message found";
      } catch {
        responseText = body || "Empty request body";
      }
    } else {
      const responseData = RESPONSES[mockType] || RESPONSES.clean;
      responseText = responseData.text;
    }

    const response = {
      id: `msg_ri_mock_${requestCount}`,
      type: "message",
      role: "assistant",
      content: [{ type: "text", text: responseText }],
      model: "ri-mock-server",
      stop_reason: "end_turn",
      usage: { input_tokens: 10, output_tokens: 50 },
    };

    res.writeHead(200, { "Content-Type": "application/json" });
    res.end(JSON.stringify(response));
  });
});

server.listen(PORT, "127.0.0.1", () => {
  console.log(`RI mock server listening on 127.0.0.1:${PORT}`);
  console.log(`Modes: clean, persuasive, commercial, fear, echo`);
  console.log(`Set mode via X-Mock-Response header`);
});

process.on("SIGTERM", () => server.close(() => process.exit(0)));
process.on("SIGINT", () => server.close(() => process.exit(0)));
