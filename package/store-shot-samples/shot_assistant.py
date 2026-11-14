"""A stand-in for the Claude API, used only to take the assistant screenshot without a key.

    python package/store-shot-samples/shot_assistant.py 8766

It plays one fixed conversation about rogeha-reader-tour.pdf: the assistant asks the reader for
the document's pages, then answers with page citations. The answer is written by hand from what
the tour says on those pages, so the screenshot shows the real window with a scripted answer.
Point the reader at it with ROGEHA_ANTHROPIC_BASE_URL=http://127.0.0.1:8766 and any API key.
"""
import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

ANSWER = (
    "Rogeha Reader keeps its memory use small in three ways [p. 7]:\n\n"
    "1. Pages that are not on screen are kept packed.\n"
    "2. Pages are stored with three bytes per dot instead of four.\n"
    "3. The window picture is held once, not twice.\n\n"
    "With a 14-page paper open on a high-resolution screen, that brought it from 98 MB down to 44 MB [p. 7]. "
    "The assistant itself is a separate program that only starts when you open its panel, so it costs "
    "nothing while you just read [p. 6]."
)


def sse(handler, event):
    handler.wfile.write(f"event: {event['type']}\ndata: {json.dumps(event)}\n\n".encode())
    handler.wfile.flush()


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("content-length", 0))))
        last = body["messages"][-1]
        answered = isinstance(last["content"], list) and any(b.get("type") == "tool_result" for b in last["content"])
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.end_headers()
        sse(self, {"type": "message_start", "message": {"id": "msg_shot", "type": "message", "role": "assistant", "content": []}})
        if not answered:
            sse(self, {"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "toolu_shot", "name": "get_pages", "input": {}}})
            sse(self, {"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": '{"from": 6, "to": 7}'}})
            sse(self, {"type": "content_block_stop", "index": 0})
            sse(self, {"type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": {"output_tokens": 20}})
        else:
            sse(self, {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}})
            for at in range(0, len(ANSWER), 60):
                sse(self, {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": ANSWER[at:at + 60]}})
            sse(self, {"type": "content_block_stop", "index": 0})
            sse(self, {"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 120}})
        sse(self, {"type": "message_stop"})


if __name__ == "__main__":
    HTTPServer(("127.0.0.1", int(sys.argv[1]) if len(sys.argv) > 1 else 8766), Handler).serve_forever()
