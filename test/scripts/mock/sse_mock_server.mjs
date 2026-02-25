#!/usr/bin/env node
// sse_mock_server.mjs — Mock upstream server that returns SSE (Server-Sent Events) responses.
//
// Simulates an LLM streaming endpoint by returning the response as SSE events.
// The response text is split across multiple data frames to test the proxy's
// SSE accumulator and cross-frame PII scanning.
//
// Usage:
//   node test/scripts/mock/sse_mock_server.mjs
//
// Environment:
//   SSE_MOCK_PORT — Listen port (default: 18792)

import { createServer } from "http";

const PORT = parseInt(process.env.SSE_MOCK_PORT || "18792");

let requestCount = 0;

const server = createServer((req, res) => {
  let body = "";
  req.on("data", (chunk) => (body += chunk));
  req.on("end", () => {
    requestCount++;

    // SSE response: text/event-stream with multiple data frames
    res.writeHead(200, {
      "Content-Type": "text/event-stream",
      "Cache-Control": "no-cache",
      Connection: "keep-alive",
    });

    // Parse the request to extract the user message for echoing
    let echoText = "Hello from the SSE mock server.";
    try {
      const parsed = JSON.parse(body);
      const userMsg = parsed.messages?.find((m) => m.role === "user");
      if (userMsg?.content) {
        echoText =
          typeof userMsg.content === "string"
            ? userMsg.content
            : userMsg.content
                .filter((c) => c.type === "text")
                .map((c) => c.text)
                .join(" ");
      }
    } catch {
      // Use default echo text
    }

    // Split echo text into chunks of ~20 chars to simulate streaming
    const chunks = [];
    for (let i = 0; i < echoText.length; i += 20) {
      chunks.push(echoText.slice(i, i + 20));
    }

    // Send message_start event
    const startEvent = {
      type: "message_start",
      message: {
        id: `msg_sse_${requestCount}`,
        type: "message",
        role: "assistant",
        model: "sse-mock-server",
        usage: { input_tokens: 10, output_tokens: 0 },
      },
    };
    res.write(`event: message_start\ndata: ${JSON.stringify(startEvent)}\n\n`);

    // Send content_block_start
    const blockStart = {
      type: "content_block_start",
      index: 0,
      content_block: { type: "text", text: "" },
    };
    res.write(
      `event: content_block_start\ndata: ${JSON.stringify(blockStart)}\n\n`
    );

    // Send content deltas
    let chunkIndex = 0;
    const sendChunk = () => {
      if (chunkIndex < chunks.length) {
        const delta = {
          type: "content_block_delta",
          index: 0,
          delta: { type: "text_delta", text: chunks[chunkIndex] },
        };
        res.write(
          `event: content_block_delta\ndata: ${JSON.stringify(delta)}\n\n`
        );
        chunkIndex++;
        setTimeout(sendChunk, 10); // Small delay between chunks
      } else {
        // Send content_block_stop
        const blockStop = { type: "content_block_stop", index: 0 };
        res.write(
          `event: content_block_stop\ndata: ${JSON.stringify(blockStop)}\n\n`
        );

        // Send message_delta (stop reason)
        const msgDelta = {
          type: "message_delta",
          delta: { stop_reason: "end_turn" },
          usage: { output_tokens: chunks.length * 5 },
        };
        res.write(
          `event: message_delta\ndata: ${JSON.stringify(msgDelta)}\n\n`
        );

        // Send message_stop
        const msgStop = { type: "message_stop" };
        res.write(
          `event: message_stop\ndata: ${JSON.stringify(msgStop)}\n\n`
        );

        // Send [DONE] termination
        res.write("data: [DONE]\n\n");
        res.end();
      }
    };

    sendChunk();
  });
});

server.listen(PORT, "127.0.0.1", () => {
  console.log(`SSE mock server listening on 127.0.0.1:${PORT}`);
  console.log(`Returns text/event-stream with chunked content deltas`);
});

process.on("SIGTERM", () => server.close(() => process.exit(0)));
process.on("SIGINT", () => server.close(() => process.exit(0)));
