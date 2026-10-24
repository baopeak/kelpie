"""A stand-in Agent Client Protocol agent (JSON-RPC 2.0, one message per line on stdio) for the
chat-view tests: it answers like a real ACP agent without any account or network.

    python scripts/e2e/fake_acp_agent.py [--no-load] [--auth] [--image] [--state FILE] [--log FILE]

Each prompt runs one tool-using turn: a thought, a tool call that asks for permission, the
tool's result, an answer, a title and a usage update, the stop reason — then one more usage
update after it (agents do that; it must not mark the session working again). A prompt that
contains "ask me" asks the user a question (elicitation) instead of the permission round.
`session/load` replays the conversation so far. `--no-load` hides `loadSession`; `--auth`
requires `authenticate` before a session opens; `--image` says prompts may carry pictures. `--state FILE` keeps the conversations in a file, so
a new process (the agent started again) loads what the last one said; `--log FILE` appends every
message the client sends to it, one JSON object per line.
"""
import json
import os
import sys
import time
import uuid

PAUSE = 0.8  # seconds between a turn's steps: the agent is visibly working meanwhile

LOAD = "--no-load" not in sys.argv
AUTH = "--auth" in sys.argv
IMAGE = "--image" in sys.argv


def option(name):
    return sys.argv[sys.argv.index(name) + 1] if name in sys.argv else None


STATE = option("--state")
LOG = option("--log")
sessions = {}  # sessionId -> list of (role, text)
if STATE and os.path.exists(STATE):
    with open(STATE, encoding="utf-8") as f:
        sessions = {k: [tuple(x) for x in v] for k, v in json.load(f).items()}
authenticated = not AUTH
pending = {}  # our request id -> continuation
next_id = 0


def send(msg):
    sys.stdout.write(json.dumps(msg) + "\n")
    sys.stdout.flush()


def save():
    if STATE:
        with open(STATE, "w", encoding="utf-8") as f:
            json.dump({k: [list(x) for x in v] for k, v in sessions.items()}, f)


def update(session_id, upd):
    send({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": session_id, "update": upd}})


def request(method, params, then):
    global next_id
    next_id += 1
    rid = f"agent-{next_id}"
    pending[rid] = then
    send({"jsonrpc": "2.0", "id": rid, "method": method, "params": params})


def text(t):
    return {"type": "text", "text": t}


def config_options(model="fast"):
    return [
        {"id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": model,
         "options": [{"value": "fast", "name": "Fast"}, {"value": "deep", "name": "Deep"}]},
        {"id": "mode", "name": "Mode", "category": "mode", "type": "select", "currentValue": "ask",
         "options": [{"value": "ask", "name": "Ask"}, {"value": "auto", "name": "Auto"}]},
    ]


def run_turn(rid, sid, prompt_text):
    sessions[sid].append(("user", prompt_text))
    update(sid, {"sessionUpdate": "agent_thought_chunk", "content": text("Thinking about it.")})
    time.sleep(PAUSE)

    def finish(answer):
        update(sid, {"sessionUpdate": "agent_message_chunk", "content": text(answer)})
        sessions[sid].append(("agent", answer))
        save()
        update(sid, {"sessionUpdate": "session_info_update", "title": "Fake task: " + prompt_text[:30]})
        update(sid, {"sessionUpdate": "usage_update", "used": 1200 * len(sessions[sid]), "size": 200000, "cost": {"amount": 0.01, "currency": "USD"}})
        send({"jsonrpc": "2.0", "id": rid, "result": {"stopReason": "end_turn"}})
        update(sid, {"sessionUpdate": "usage_update", "used": 1200 * len(sessions[sid]) + 1, "size": 200000})

    if "ask me" in prompt_text:
        def answered(res):
            content = (res or {}).get("content") or {}
            finish(f"You said: {content.get('answer', res.get('action') if isinstance(res, dict) else res)}")
        request("elicitation/create", {"sessionId": sid, "mode": "form", "message": "Which colour?", "requestedSchema": {
            "type": "object", "properties": {"answer": {"type": "string", "title": "Colour"}}, "required": ["answer"]}}, answered)
        return

    tool = "call-" + uuid.uuid4().hex[:8]
    update(sid, {"sessionUpdate": "tool_call", "toolCallId": tool, "title": "Read notes.txt", "kind": "read",
                 "status": "pending", "locations": [{"path": "notes.txt"}], "rawInput": {"path": "notes.txt"}})

    def permitted(res):
        outcome = ((res or {}).get("outcome") or {})
        if outcome.get("outcome") != "selected" or not str(outcome.get("optionId", "")).startswith("allow"):
            update(sid, {"sessionUpdate": "tool_call_update", "toolCallId": tool, "status": "failed"})
            return finish("Not allowed, so I stopped.")
        update(sid, {"sessionUpdate": "tool_call_update", "toolCallId": tool, "status": "completed",
                     "content": [{"type": "content", "content": text("hello from notes")}]})
        time.sleep(PAUSE)
        finish("Done: the file says hello.")

    request("session/request_permission", {"sessionId": sid, "toolCall": {"toolCallId": tool, "title": "Read notes.txt", "rawInput": {"path": "notes.txt"}},
                                           "options": [{"optionId": "allow-once", "name": "Allow", "kind": "allow_once"},
                                                       {"optionId": "reject-once", "name": "Reject", "kind": "reject_once"}]}, permitted)


for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    msg = json.loads(line)
    if LOG:
        with open(LOG, "a", encoding="utf-8") as f:
            f.write(line + "\n")
    method, rid, params = msg.get("method"), msg.get("id"), msg.get("params") or {}
    if method is None and rid in pending:  # a response to our request
        pending.pop(rid)(msg.get("result"))
        continue
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": rid, "result": {
            "protocolVersion": 1,
            "agentCapabilities": {"loadSession": LOAD, "promptCapabilities": {"image": IMAGE, "embeddedContext": True}},
            "authMethods": [{"id": "fake-login", "name": "Fake login", "description": "Always succeeds"}] if AUTH else [],
            "agentInfo": {"name": "fake-acp", "version": "1.0.0"}}})
    elif method == "authenticate":
        authenticated = True
        send({"jsonrpc": "2.0", "id": rid, "result": {}})
    elif method in ("session/new", "session/load"):
        if not authenticated:
            send({"jsonrpc": "2.0", "id": rid, "error": {"code": -32000, "message": "Authentication required"}})
            continue
        if method == "session/new":
            sid = "fake-" + uuid.uuid4().hex[:12]
            sessions[sid] = []
            save()
        else:
            sid = params.get("sessionId")
            if sid not in sessions:
                sessions[sid] = [("user", "earlier question"), ("agent", "earlier answer")]
                save()
            for role, t in sessions[sid]:
                update(sid, {"sessionUpdate": "user_message_chunk" if role == "user" else "agent_message_chunk", "content": text(t)})
        update(sid, {"sessionUpdate": "available_commands_update", "availableCommands": [{"name": "review", "description": "Review the changes"}]})
        send({"jsonrpc": "2.0", "id": rid, "result": {"sessionId": sid, "configOptions": config_options()}})
    elif method == "session/prompt":
        sid = params.get("sessionId")
        sessions.setdefault(sid, [])
        save()
        prompt_text = " ".join(p.get("text", "") for p in params.get("prompt", []) if p.get("type") == "text")
        run_turn(rid, sid, prompt_text)
    elif method == "session/set_config_option":
        send({"jsonrpc": "2.0", "id": rid, "result": {"configOptions": config_options(params.get("value", "fast"))}})
    elif method == "session/cancel":
        pass
    elif rid is not None:
        send({"jsonrpc": "2.0", "id": rid, "error": {"code": -32601, "message": f"fake agent: no {method}"}})
