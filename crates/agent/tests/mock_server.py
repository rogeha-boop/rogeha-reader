"""A local stand-in for the Claude Messages API and an OpenAI-compatible server.

Used to check rogeha-agent without an API key or network access:

    python crates/agent/tests/mock_server.py 8765 requests.log

* POST /v1/messages          Claude: streamed (SSE). First round asks for a tool, the next
                             round answers with page citations. Key "bad-key" gets a 401.
* POST /v1/chat/completions  OpenAI style: the same two rounds, not streamed.

Every request (headers and body) is appended to the log file as one JSON line, so a test can
check what the agent actually sent.
"""
import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

LOG = sys.argv[2] if len(sys.argv) > 2 else "requests.log"


def sse(handler, event):
    handler.wfile.write(f"event: {event['type']}\ndata: {json.dumps(event)}\n\n".encode())
    handler.wfile.flush()


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("content-length", 0))))
        with open(LOG, "a", encoding="utf-8") as f:
            f.write(json.dumps({"path": self.path, "headers": {k.lower(): v for k, v in self.headers.items()}, "body": body}) + "\n")
        if self.path == "/v1/messages":
            self.claude(body)
        elif self.path == "/v1/chat/completions":
            self.openai(body)
        else:
            self.send_error(404)

    def claude(self, body):
        if self.headers.get("x-api-key") == "bad-key":
            self.send_response(401)
            self.send_header("content-type", "application/json")
            self.end_headers()
            self.wfile.write(json.dumps({"type": "error", "error": {"type": "authentication_error", "message": "invalid x-api-key"}}).encode())
            return
        last = body["messages"][-1]
        answered = isinstance(last["content"], list) and any(b.get("type") == "tool_result" for b in last["content"])
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.end_headers()
        sse(self, {"type": "message_start", "message": {"id": "msg_mock", "type": "message", "role": "assistant", "content": []}})
        if "landscape" in str(body["messages"][0]["content"]):
            return self.rotate_scenario(body)
        if not answered:
            # A thinking block (must come back unchanged), some text, then a tool call whose
            # input arrives in pieces.
            sse(self, {"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": "", "signature": ""}})
            sse(self, {"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": "sig-abc123"}})
            sse(self, {"type": "content_block_stop", "index": 0})
            sse(self, {"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": ""}})
            for piece in ["Let me read ", "the first pages."]:
                sse(self, {"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": piece}})
            sse(self, {"type": "content_block_stop", "index": 1})
            sse(self, {"type": "content_block_start", "index": 2, "content_block": {"type": "tool_use", "id": "toolu_mock1", "name": "get_pages", "input": {}}})
            for piece in ['{"fr', 'om": 1, ', '"to": 2}']:
                sse(self, {"type": "content_block_delta", "index": 2, "delta": {"type": "input_json_delta", "partial_json": piece}})
            sse(self, {"type": "content_block_stop", "index": 2})
            sse(self, {"type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": {"output_tokens": 30}})
        else:
            result = next(b for b in last["content"] if b.get("type") == "tool_result")
            seen = "Page 1" in str(result.get("content"))
            sse(self, {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}})
            text = "The document introduces its subject on the first page [p. 1] and continues with related work [p. 2]." if seen else "I could not read the pages."
            for piece in [text[:40], text[40:]]:
                sse(self, {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": piece}})
            sse(self, {"type": "content_block_stop", "index": 0})
            sse(self, {"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 25}})
        sse(self, {"type": "message_stop"})

    def rotate_scenario(self, body):
        """'Rotate all landscape pages': look at the document, rotate what it reports, confirm."""
        results = [b for m in body["messages"] if isinstance(m["content"], list) for b in m["content"] if b.get("type") == "tool_result"]

        def tool(name, tool_input):
            sse(self, {"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": f"toolu_{name}", "name": name, "input": {}}})
            sse(self, {"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": json.dumps(tool_input)}})
            sse(self, {"type": "content_block_stop", "index": 0})
            sse(self, {"type": "message_delta", "delta": {"stop_reason": "tool_use"}})

        if len(results) == 0:
            tool("get_document_info", {})
        elif len(results) == 1:
            pages = json.loads(results[0]["content"])["landscape_pages"]
            tool("rotate_pages", {"pages": pages, "degrees": 90})
        else:
            sse(self, {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}})
            sse(self, {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Done: " + str(results[1]["content"])}})
            sse(self, {"type": "content_block_stop", "index": 0})
            sse(self, {"type": "message_delta", "delta": {"stop_reason": "end_turn"}})
        sse(self, {"type": "message_stop"})

    def openai(self, body):
        last = body["messages"][-1]
        if last["role"] != "tool":
            message = {"role": "assistant", "content": None, "tool_calls": [{"id": "call_mock1", "type": "function", "function": {"name": "search", "arguments": json.dumps({"query": "dataset"})}}]}
        else:
            message = {"role": "assistant", "content": f"The search result was: {last['content'][:60]} [p. 3]"}
        data = json.dumps({"id": "chatcmpl-mock", "choices": [{"index": 0, "message": message, "finish_reason": "stop"}]}).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


if __name__ == "__main__":
    HTTPServer(("127.0.0.1", int(sys.argv[1]) if len(sys.argv) > 1 else 8765), Handler).serve_forever()
