"""Drive rogeha-agent against the mock server and check the whole conversation loop.

    python crates/agent/tests/check_agent.py target/release/rogeha-agent.exe

Starts mock_server.py itself, plays the reader's part (answers tool calls), and checks both
what the agent says to the reader and what it sends to the API.
"""
import json
import os
import subprocess
import sys
import tempfile
import time

agent_exe = os.path.abspath(sys.argv[1])
here = os.path.dirname(os.path.abspath(__file__))
work = tempfile.mkdtemp(prefix="rogeha-agent-test-")
log = os.path.join(work, "requests.log")
PORT = 8765
server = subprocess.Popen([sys.executable, os.path.join(here, "mock_server.py"), str(PORT), log])
time.sleep(0.8)


def start(key):
    env = dict(os.environ, ROGEHA_DATA_DIR=work, ROGEHA_ANTHROPIC_BASE_URL=f"http://127.0.0.1:{PORT}", ANTHROPIC_API_KEY=key, OPENAI_API_KEY="")
    return subprocess.Popen([agent_exe], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, encoding="utf-8", env=env)


def send(p, obj):
    p.stdin.write(json.dumps(obj) + "\n")
    p.stdin.flush()


def turn(p, text, tool_answers):
    """Ask, answer tool calls from `tool_answers`, return (answer text, tool calls, final event)."""
    send(p, {"type": "ask", "text": text, "context": "[Document: test.pdf, 14 pages, on page 1]"})
    answer, calls = "", []
    while True:
        event = json.loads(p.stdout.readline())
        if event["type"] == "delta":
            answer += event["text"]
        elif event["type"] == "tool_call":
            calls.append(event)
            send(p, {"type": "tool_result", "id": event["id"], "content": tool_answers[event["name"]], "is_error": False})
        else:
            return answer, calls, event


try:
    # --- Claude -------------------------------------------------------------------------
    p = start("test-key")
    send(p, {"type": "hello"})
    ready = json.loads(p.stdout.readline())
    assert ready["type"] == "ready" and ready["provider"] == "anthropic" and ready["model"] == "claude-opus-5-5" and ready["has_key"], ready

    answer, calls, final = turn(p, "Summarize this", {"get_pages": "=== Page 1 ===\nIntro text\n=== Page 2 ===\nRelated work"})
    assert final == {"type": "done"}, final
    assert [(c["name"], c["input"]) for c in calls] == [("get_pages", {"from": 1, "to": 2})], calls
    assert "Let me read the first pages." in answer and "[p. 1]" in answer and "[p. 2]" in answer, answer

    requests = [json.loads(line) for line in open(log, encoding="utf-8")]
    first, second = requests[0], requests[1]
    h, b = first["headers"], first["body"]
    assert first["path"] == "/v1/messages"
    assert h["x-api-key"] == "test-key" and h["anthropic-version"] == "2026-10-21", h
    assert h["anthropic-beta"] == "server-side-fallback-2026-07-01" and b["fallbacks"] == "default"
    assert b["model"] == "claude-opus-5-5" and b["stream"] is True and b["max_tokens"] == 64000
    assert b["output_config"] == {"effort": "medium"} and b["cache_control"] == {"type": "ephemeral"}
    assert "thinking" not in b and "temperature" not in b and "tool_choice" not in b
    assert len(b["tools"]) == 11 and all(t["eager_input_streaming"] is True for t in b["tools"])
    assert b["messages"][0]["content"].startswith("[Document: test.pdf") and b["messages"][0]["content"].endswith("Summarize this")
    # Second round: the assistant turn is replayed unchanged (thinking block with its
    # signature first), followed by one user message holding the tool result.
    m = second["body"]["messages"]
    assert [x["role"] for x in m] == ["user", "assistant", "user"], m
    assert m[1]["content"][0] == {"type": "thinking", "thinking": "", "signature": "sig-abc123"}, m[1]["content"][0]
    assert m[1]["content"][2] == {"type": "tool_use", "id": "toolu_mock1", "name": "get_pages", "input": {"from": 1, "to": 2}}
    assert m[2]["content"] == [{"type": "tool_result", "tool_use_id": "toolu_mock1", "content": "=== Page 1 ===\nIntro text\n=== Page 2 ===\nRelated work", "is_error": False}]

    # A follow-up question keeps the history; reset forgets it.
    turn(p, "And then?", {"get_pages": "=== Page 1 ===\nIntro"})
    requests = [json.loads(line) for line in open(log, encoding="utf-8")]
    assert len(requests[2]["body"]["messages"]) == 5, len(requests[2]["body"]["messages"])
    send(p, {"type": "reset"})
    assert json.loads(p.stdout.readline())["type"] == "ready"
    turn(p, "Fresh start", {"get_pages": "=== Page 1 ===\nIntro"})
    requests = [json.loads(line) for line in open(log, encoding="utf-8")]
    assert len(requests[4]["body"]["messages"]) == 1
    p.kill()

    # --- a wrong key gives a clear message, and the failed turn is not kept ---------------
    p = start("bad-key")
    send(p, {"type": "hello"}); p.stdout.readline()
    _, _, final = turn(p, "Hi", {})
    assert final["type"] == "error" and "API key" in final["text"], final
    p.kill()

    # --- no key at all: asked to add one, nothing is sent ---------------------------------
    before = sum(1 for _ in open(log, encoding="utf-8"))
    p = start("")
    send(p, {"type": "hello"})
    ready = json.loads(p.stdout.readline())
    if not ready["has_key"]:  # (a key saved in the OS credential store would count)
        _, _, final = turn(p, "Hi", {})
        assert final["type"] == "error" and "key" in final["text"].lower(), final
        assert sum(1 for _ in open(log, encoding="utf-8")) == before
    p.kill()

    # --- OpenAI-compatible (a local server needs no key) ---------------------------------
    p = start("")
    send(p, {"type": "configure", "provider": "openai", "model": "llama-test", "base_url": f"http://127.0.0.1:{PORT}/v1", "effort": "low"})
    ready = json.loads(p.stdout.readline())
    assert ready["provider"] == "openai" and ready["model"] == "llama-test", ready
    answer, calls, final = turn(p, "Where are datasets mentioned?", {"search": "3 matches on page 3"})
    assert final == {"type": "done"} and [(c["name"], c["input"]) for c in calls] == [("search", {"query": "dataset"})], (final, calls)
    assert "3 matches on page 3" in answer and "[p. 3]" in answer, answer
    last = [json.loads(line) for line in open(log, encoding="utf-8")][-1]
    assert last["path"] == "/v1/chat/completions" and "authorization" not in last["headers"]
    roles = [x["role"] for x in last["body"]["messages"]]
    assert roles == ["system", "user", "assistant", "tool"], roles
    assert last["body"]["tools"][0]["type"] == "function"
    p.kill()

    # The settings were remembered (without any key in the file).
    saved = json.load(open(os.path.join(work, "assistant.json"), encoding="utf-8"))
    assert saved == {"provider": "openai", "model": "llama-test", "base_url": f"http://127.0.0.1:{PORT}/v1", "effort": "low"}, saved
    print("OK: Claude streaming + tool loop, thinking replay, errors, reset, OpenAI-compatible loop")
finally:
    server.kill()
