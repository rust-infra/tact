#!/usr/bin/env python3
"""Measure DeepSeek prefix-cache effects of `reasoning_content` replay policies.

DeepSeek's thinking mode returns the chain of thought in `reasoning_content`.
When a request carries `tools`, the official docs say every earlier turn's
`reasoning_content` must be passed back; this script measures what the three
common client policies actually cost against the Context Cache:

  full    replay every assistant's `reasoning_content` (docs-compliant)
  omit    replay none (Tact's current DeepSeek behavior)
  latest  replay only the most recent assistant's `reasoning_content`

Each policy runs a short thinking-mode tool-calling conversation and prints
prompt / cache-hit / cache-miss / completion tokens per turn plus the hit
sequence. Cache hit and miss prices differ by ~50x, so the hit ratio decides
the cost ranking; see book/26_chapter_issue_zh.md (2026-10-03).

Usage:
    export DEEPSEEK_API_KEY=sk-...        # or reuse ~/.tact/config.toml
    python3 scripts/deepseek-reasoning-cache-probe.py
    DS_MODEL=deepseek-flash DS_TURNS=6 \
        python3 scripts/deepseek-reasoning-cache-probe.py

Env:
    DEEPSEEK_API_KEY    API key; overrides ~/.tact/config.toml
    DEEPSEEK_BASE_URL   API base; overrides config/default
    DS_MODEL            force a model (default: first thinking-capable of
                        deepseek-v4-flash, deepseek-flash, deepseek-chat)
    DS_TURNS            turns per policy (default: 4)

Requires Python 3.11+ (tomllib). Never prints the API key.
"""

import copy
import json
import os
import time
import tomllib
import urllib.error
import urllib.request

DEFAULT_MODELS = ["deepseek-v4-flash", "deepseek-flash", "deepseek-chat"]
POLICIES = ["full", "omit", "latest"]
RUN_NONCE = os.urandom(4).hex()
TURNS = int(os.environ.get("DS_TURNS", "4"))
TOOLS = [
    {
        "type": "function",
        "function": {
            "name": "get_date",
            "description": "Get a date as YYYY-mm-dd",
            "parameters": {"type": "object", "properties": {}},
        },
    }
]


def load_config():
    path = os.path.join(os.path.expanduser("~"), ".tact", "config.toml")
    try:
        with open(path, "rb") as fh:
            return tomllib.load(fh)
    except FileNotFoundError:
        return {}


def resolve_provider():
    cfg = load_config()
    entry = (cfg.get("llm", {}).get("providers", {}).get("deepseek", {})) or {}
    key = os.environ.get("DEEPSEEK_API_KEY") or entry.get("api_key")
    source = "env DEEPSEEK_API_KEY" if os.environ.get("DEEPSEEK_API_KEY") else "~/.tact/config.toml"
    if not key or len(key) < 20:
        raise SystemExit(
            f"DeepSeek key looks like a placeholder (len={len(key) if key else 0}, source={source}); "
            "set a real DEEPSEEK_API_KEY or [llm.providers.deepseek].api_key"
        )
    base = (
        os.environ.get("DEEPSEEK_BASE_URL")
        or entry.get("base_url")
        or "https://api.deepseek.com"
    ).rstrip("/")
    print(f"[key] source={source} len={len(key)}")
    return key, base


KEY, BASE = resolve_provider()


def http(path, body):
    req = urllib.request.Request(
        BASE + path,
        data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json", "Authorization": f"Bearer {KEY}"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=180) as resp:
            return resp.status, json.load(resp)
    except urllib.error.HTTPError as err:
        raw = err.read().decode(errors="replace")
        try:
            return err.code, json.loads(raw)
        except Exception:
            return err.code, {"raw": raw[:500]}


def pick_model():
    preferred = os.environ.get("DS_MODEL")
    candidates = ([preferred] if preferred else []) + DEFAULT_MODELS
    seen = set()
    for model in candidates:
        if not model or model in seen:
            continue
        seen.add(model)
        status, resp = http(
            "/chat/completions",
            {
                "model": model,
                "messages": [{"role": "user", "content": "Reply with exactly: pong"}],
                "thinking": {"type": "enabled"},
                "reasoning_effort": "high",
                "max_tokens": 32,
            },
        )
        msg = (resp.get("choices") or [{}])[0].get("message", {})
        ok = status == 200 and isinstance(msg.get("reasoning_content"), str)
        print(f"[pick] {model}: status={status} rc={'yes' if ok else 'no'}")
        if ok:
            return model
    raise SystemExit("no thinking-capable model found")


def call(model, messages, label):
    status, resp = http(
        "/chat/completions",
        {
            "model": model,
            "messages": messages,
            "tools": TOOLS,
            "thinking": {"type": "enabled"},
            "reasoning_effort": "high",
            "max_tokens": 512,
            "stream": False,
        },
    )
    usage = resp.get("usage") or {}
    msg = ((resp.get("choices") or [{}])[0]).get("message") or {}
    reasoning = msg.get("reasoning_content")
    tool_calls = msg.get("tool_calls") or []
    print(
        f"{label:34s} status={status} prompt={usage.get('prompt_tokens')} "
        f"hit={usage.get('prompt_cache_hit_tokens')} "
        f"miss={usage.get('prompt_cache_miss_tokens')} "
        f"completion={usage.get('completion_tokens')} "
        f"rc_len={len(reasoning) if isinstance(reasoning, str) else 0} tools={len(tool_calls)}"
    )
    if status != 200:
        print("   error:", json.dumps(resp)[:300])
    return status, msg, usage


def transform(messages, policy):
    out = copy.deepcopy(messages)
    if policy == "full":
        return out
    if policy == "omit":
        for msg in out:
            msg.pop("reasoning_content", None)
        return out
    if policy == "latest":
        last = None
        for i, msg in enumerate(out):
            if msg.get("role") == "assistant" and msg.get("reasoning_content"):
                last = i
        for i, msg in enumerate(out):
            if i != last:
                msg.pop("reasoning_content", None)
        return out
    raise ValueError(policy)


def run_policy(policy, model, turns):
    print(f"\n=== policy: {policy} ===")
    messages = [
        {
            "role": "system",
            "content": (
                f"You are a terse tool-using assistant. Cache-policy marker: {policy}. "
                f"Run nonce: {RUN_NONCE}. Whenever the user asks for a date, you must call "
                "the get_date tool. Never answer a date from memory."
            ),
        },
        {
            "role": "user",
            "content": "Call the get_date tool now. Do not answer from memory — only via the tool.",
        },
    ]
    hits = []
    for turn in range(1, turns + 1):
        time.sleep(3)  # let the disk cache persist before the next request
        status, msg, usage = call(model, transform(messages, policy), f"{policy} turn{turn}")
        hits.append(usage.get("prompt_cache_hit_tokens"))
        messages.extend(transform([msg], policy))
        tool_calls = msg.get("tool_calls") or []
        if tool_calls:
            messages.append(
                {
                    "role": "tool",
                    "tool_call_id": tool_calls[0].get("id", "call_0"),
                    "content": "2026-10-03",
                }
            )
        if turn < turns:
            content = (
                "Call get_date again. Use the tool; do not reuse the previous answer."
                if tool_calls
                else "You must call the get_date tool. Do it now."
            )
            messages.append({"role": "user", "content": content})
        if status != 200:
            break
    print(f"=== {policy} hit sequence: {hits} ===")


if __name__ == "__main__":
    model = pick_model()
    for policy in POLICIES:
        run_policy(policy, model, TURNS)
