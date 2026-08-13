#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""DeepSeek API 协议探测脚本。

调用真实 DeepSeek API，把完整的请求 / 响应原样打印出来，用来确认：
  1. /models           —— 账号能用哪些模型
  2. Responses（非流式） —— 完整响应 JSON 的字段
  3. Responses（流式 SSE） —— 语义事件形态
  4. function calling —— DeepSeek 返回的 function_call 精确结构
  5. function_call_output 回传 —— 验证 Responses item wire 格式
  6. custom apply_patch —— 验证 custom tool 与语义 SSE
  7. native web_search —— 验证 DeepSeek 原生搜索生命周期

只用 Python 标准库（urllib），无需 pip install。

API Key 获取顺序：
  --key sk-xxx  >  环境变量 DEEPSEEK_API_KEY  >  尝试 Windows 凭据管理器（python keyring）

用法：
  python scripts/probe_deepseek.py --key sk-xxxx
  set DEEPSEEK_API_KEY=sk-xxxx & python scripts/probe_deepseek.py
  python scripts/probe_deepseek.py --only 5      # 只跑第 5 步（400 复现/验证）
"""

import argparse
import json
import sys
import urllib.error
import urllib.request

BASE_URL = "https://api.deepseek.com"
KEYCHAIN_SERVICE = "deepagent-studio"
KEYCHAIN_NAME = "deepseek_api_key"

# 终端颜色（Windows 10+ 终端支持 ANSI）
C_RESET = "\033[0m"
C_DIM = "\033[2m"
C_CYAN = "\033[36m"
C_GREEN = "\033[32m"
C_YELLOW = "\033[33m"
C_RED = "\033[31m"
C_BOLD = "\033[1m"

TERMINAL_EVENTS = {"response.completed", "response.incomplete", "response.failed"}


def hr(title: str) -> None:
    print(f"\n{C_BOLD}{C_CYAN}{'=' * 70}{C_RESET}")
    print(f"{C_BOLD}{C_CYAN}  {title}{C_RESET}")
    print(f"{C_BOLD}{C_CYAN}{'=' * 70}{C_RESET}")


def pretty(label: str, obj) -> None:
    print(f"{C_DIM}{label}:{C_RESET}")
    print(json.dumps(obj, ensure_ascii=False, indent=2))


def resolve_key(cli_key: str | None) -> str:
    if cli_key:
        return cli_key.strip()
    import os

    env = os.environ.get("DEEPSEEK_API_KEY", "").strip()
    if env:
        print(f"{C_DIM}使用环境变量 DEEPSEEK_API_KEY{C_RESET}")
        return env
    # 尝试 python keyring（注意：Rust keyring 与 python keyring 的凭据目标名
    # 可能不一致，读不到属正常，回退到 --key / 环境变量即可）
    try:
        import keyring  # type: ignore

        val = keyring.get_password(KEYCHAIN_SERVICE, KEYCHAIN_NAME)
        if val:
            print(f"{C_DIM}从 Windows 凭据管理器读到 key{C_RESET}")
            return val.strip()
    except Exception as exc:  # noqa: BLE001
        print(f"{C_DIM}（python keyring 读取失败，忽略：{exc}）{C_RESET}")
    # keyring v3 on Windows stores a Generic Credential whose target is
    # `<name>.<service>`. Read it directly through the documented Win32 API;
    # the secret remains in memory and is never printed or written to disk.
    if sys.platform == "win32":
        try:
            import ctypes
            from ctypes import wintypes

            class CREDENTIAL(ctypes.Structure):
                _fields_ = [
                    ("Flags", wintypes.DWORD), ("Type", wintypes.DWORD),
                    ("TargetName", wintypes.LPWSTR), ("Comment", wintypes.LPWSTR),
                    ("LastWritten", wintypes.FILETIME), ("CredentialBlobSize", wintypes.DWORD),
                    ("CredentialBlob", ctypes.c_void_p), ("Persist", wintypes.DWORD),
                    ("AttributeCount", wintypes.DWORD), ("Attributes", ctypes.c_void_p),
                    ("TargetAlias", wintypes.LPWSTR), ("UserName", wintypes.LPWSTR),
                ]
            pcred = ctypes.POINTER(CREDENTIAL)()
            target = f"{KEYCHAIN_NAME}.{KEYCHAIN_SERVICE}"
            if ctypes.windll.advapi32.CredReadW(target, 1, 0, ctypes.byref(pcred)):
                try:
                    item = pcred.contents
                    raw = ctypes.string_at(item.CredentialBlob, item.CredentialBlobSize)
                    val = raw.decode("utf-16-le").rstrip("\x00")
                    if val.strip():
                        print(f"{C_DIM}从 Windows Credential Manager 读到 key{C_RESET}")
                        return val.strip()
                finally:
                    ctypes.windll.advapi32.CredFree(pcred)
        except Exception as exc:  # noqa: BLE001
            print(f"{C_DIM}（Win32 凭据读取失败，忽略：{exc}）{C_RESET}")
    print(
        f"{C_RED}未找到 API Key。请用 --key sk-xxx 传入，"
        f"或设置环境变量 DEEPSEEK_API_KEY。{C_RESET}"
    )
    sys.exit(1)


def post(path: str, key: str, payload: dict, stream: bool = False):
    url = f"{BASE_URL}{path}"
    data = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(url, data=data, method="POST")
    req.add_header("Content-Type", "application/json")
    req.add_header("Authorization", f"Bearer {key}")
    req.add_header("Accept", "text/event-stream" if stream else "application/json")
    return urllib.request.urlopen(req, timeout=120)


def get(path: str, key: str):
    url = f"{BASE_URL}{path}"
    req = urllib.request.Request(url, method="GET")
    req.add_header("Authorization", f"Bearer {key}")
    return urllib.request.urlopen(req, timeout=60)


def read_error(exc: urllib.error.HTTPError) -> None:
    body = exc.read().decode("utf-8", errors="replace")
    print(f"{C_RED}HTTP {exc.code} {exc.reason}{C_RESET}")
    try:
        pretty("error body", json.loads(body))
    except Exception:  # noqa: BLE001
        print(body)


# ----------------------------------------------------------------------------
# 探测步骤
# ----------------------------------------------------------------------------


def probe_models(key: str) -> str:
    hr("1. GET /models —— 可用模型列表")
    try:
        with get("/models", key) as resp:
            body = json.loads(resp.read().decode("utf-8"))
        pretty("response", body)
        models = [m.get("id") for m in body.get("data", [])]
        print(f"{C_GREEN}可用模型: {models}{C_RESET}")
        # 选一个对话模型
        for pref in ("deepseek-chat", "deepseek-reasoner"):
            if pref in models:
                return pref
        return models[0] if models else "deepseek-chat"
    except urllib.error.HTTPError as exc:
        read_error(exc)
        return "deepseek-chat"


def probe_chat_nonstream(key: str, model: str) -> None:
    hr("2. POST /responses（非流式）—— 完整响应 JSON")
    payload = {
        "model": model,
        "instructions": "你是一个简洁的助手。",
        "input": [{"role": "user", "content": "用一句话介绍你自己。"}],
        "stream": False,
    }
    pretty("request", payload)
    try:
        with post("/responses", key, payload) as resp:
            body = json.loads(resp.read().decode("utf-8"))
        pretty("response", body)
        print(f"{C_GREEN}response 字段: {list(body.keys())}{C_RESET}")
        print(f"{C_GREEN}output items: {len(body.get('output', []))}{C_RESET}")
    except urllib.error.HTTPError as exc:
        read_error(exc)


def probe_chat_stream(key: str, model: str) -> None:
    hr("3. POST /responses（语义 SSE）—— 每个 event 形态")
    payload = {
        "model": model,
        "input": [{"type": "message", "role": "user", "content": "从 1 数到 3。"}],
        "stream": True,
    }
    pretty("request", payload)
    try:
        with post("/responses", key, payload, stream=True) as resp:
            count = 0
            for raw in resp:
                line = raw.decode("utf-8", errors="replace").rstrip("\n")
                if not line.strip():
                    continue
                if not line.startswith("data:"):
                    print(f"{C_YELLOW}非 data 行: {line}{C_RESET}")
                    continue
                data = line[len("data:"):].strip()
                count += 1
                event_type = None
                if count <= 8:  # 只详细打印前几个
                    try:
                        event = json.loads(data)
                        event_type = event.get("type")
                        pretty(f"chunk #{count}", event)
                    except Exception:  # noqa: BLE001
                        print(f"chunk #{count}: {data}")
                else:
                    try:
                        event_type = json.loads(data).get("type")
                    except Exception:  # noqa: BLE001
                        event_type = None
                if event_type in TERMINAL_EVENTS:
                    print(f"{C_GREEN}终态事件: {event_type}{C_RESET}")
                    break
            print(f"{C_GREEN}共收到 {count} 个 data chunk{C_RESET}")
    except urllib.error.HTTPError as exc:
        read_error(exc)


def _weather_tool() -> dict:
    return {
        "type": "function",
        "name": "get_weather",
        "description": "查询某地天气",
        "parameters": {
            "type": "object",
            "properties": {"city": {"type": "string", "description": "城市名"}},
            "required": ["city"],
        },
    }


def probe_tool_request(key: str, model: str):
    hr("4. function calling —— Responses function_call 精确结构")
    payload = {
        "model": model,
        "input": [{"type": "message", "role": "user", "content": "北京今天天气怎么样？"}],
        "tools": [_weather_tool()],
        "stream": False,
    }
    pretty("request", payload)
    try:
        with post("/responses", key, payload) as resp:
            body = json.loads(resp.read().decode("utf-8"))
        pretty("response", body)
        calls = [item for item in body.get("output", []) if item.get("type") == "function_call"]
        if calls:
            print(f"{C_GREEN}{C_BOLD}DeepSeek 返回的 Responses function_call 结构:{C_RESET}")
            pretty("output[function_call]", calls[0])
            print(f"{C_YELLOW}=> 顶层字段: {list(calls[0].keys())}{C_RESET}")
        return {"output": calls}
    except urllib.error.HTTPError as exc:
        read_error(exc)
        return None


def probe_tool_roundtrip(key: str, model: str, assistant_msg: dict | None) -> None:
    hr("5. function_call_output 回传 —— 验证 Responses item 格式")
    # 若上一步没拿到，就手工构造一个标准 assistant tool_calls 消息
    if not assistant_msg or not assistant_msg.get("output"):
        print(f"{C_DIM}（无上一步结果，使用标准构造的 assistant 消息）{C_RESET}")
        assistant_msg = {"output": [{"type": "function_call", "call_id": "call_demo_1", "name": "get_weather", "arguments": '{"city": "北京"}'}]}
    call_id = assistant_msg["output"][0]["call_id"]

    # --- 5a. 正确格式：tool_call 含 type + function{name, arguments(字符串)} ---
    print(f"\n{C_BOLD}5a. 正确格式（含 type:function）{C_RESET}")
    ok_payload = {
        "model": model,
        "input": assistant_msg["output"] + [{"type": "function_call_output", "call_id": call_id, "output": '{"city": "北京", "weather": "晴", "temp": 25}'}],
        "tools": [_weather_tool()],
        "stream": False,
    }
    pretty("request", ok_payload)
    try:
        with post("/responses", key, ok_payload) as resp:
            body = json.loads(resp.read().decode("utf-8"))
        print(f"{C_GREEN}成功！最终回复:{C_RESET}")
        # Non-streaming Responses puts final text inside output[] message
        # content parts; there is no top-level `output_text` field (that is an
        # OpenAI SDK convenience). Extract it the wire-accurate way.
        final = None
        for item in body.get("output", []):
            if item.get("type") == "message":
                for part in item.get("content", []):
                    if part.get("type") == "output_text":
                        final = (final or "") + part.get("text", "")
        print(final if final is not None else body.get("output_text"))
    except urllib.error.HTTPError as exc:
        read_error(exc)

    # --- 5b. 错误格式：arguments 对象（Responses 要求 JSON 字符串）---
    print(f"\n{C_BOLD}5b. 错误格式（arguments 对象）—— 验证 400{C_RESET}")
    bad_payload = {
        "model": model,
        "input": [{"type": "function_call", "call_id": call_id, "name": "get_weather", "arguments": {"city": "北京"}}, {"type": "function_call_output", "call_id": call_id, "output": '{"weather": "晴"}'}],
        "tools": [_weather_tool()],
        "stream": False,
    }
    try:
        with post("/responses", key, bad_payload):
            print(f"{C_YELLOW}意外成功：provider 行为可能已变化{C_RESET}")
    except urllib.error.HTTPError as exc:
        print(f"{C_GREEN}如预期失败：HTTP {exc.code}{C_RESET}")


def probe_custom_apply_patch(key: str, model: str) -> None:
    hr("6. custom apply_patch —— 验证 custom tool SSE")
    payload = {
        "model": model,
        "input": [{"type": "message", "role": "user", "content": "You must use apply_patch to propose adding one newline to demo.txt; do not answer in prose."}],
        "tools": [{
            "type": "custom", "name": "apply_patch",
            "description": "Return a patch as plain text", "format": {"type": "text"},
        }],
        "stream": True,
    }
    try:
        seen = []
        with post("/responses", key, payload, stream=True) as resp:
            for raw in resp:
                line = raw.decode("utf-8", errors="replace").strip()
                if not line.startswith("data:"):
                    continue
                data = line[len("data:"):].strip()
                if not data:
                    continue
                event = json.loads(data)
                seen.append(event.get("type"))
                if event.get("type") in TERMINAL_EVENTS:
                    break
        required = {"response.custom_tool_call_input.delta", "response.custom_tool_call_input.done", "response.completed"}
        print(f"{C_GREEN}custom tool 事件齐全: {required.issubset(set(seen))}{C_RESET}")
    except urllib.error.HTTPError as exc:
        read_error(exc)

def probe_native_web_search(key: str, model: str) -> None:
    hr("7. native web_search —— 验证 item_id 生命周期")
    payload = {
        "model": model,
        "input": [{"type": "message", "role": "user", "content": "Search for today's date and answer briefly."}],
        "tools": [{"type": "web_search"}],
        "stream": True,
    }
    try:
        lifecycle = []
        with post("/responses", key, payload, stream=True) as resp:
            for raw in resp:
                line = raw.decode("utf-8", errors="replace").strip()
                if not line.startswith("data:"):
                    continue
                data = line[len("data:"):].strip()
                if not data:
                    continue
                event = json.loads(data)
                if event.get("type", "").startswith("response.web_search_call."):
                    lifecycle.append((event.get("type"), bool(event.get("item_id"))))
                if event.get("type") in TERMINAL_EVENTS:
                    break
        print(f"{C_GREEN}web_search 生命周期: {lifecycle}{C_RESET}")
    except urllib.error.HTTPError as exc:
        read_error(exc)

def main() -> None:
    parser = argparse.ArgumentParser(description="DeepSeek API 协议探测")
    parser.add_argument("--key", help="DeepSeek API Key（sk-...）")
    parser.add_argument(
        "--only",
        type=int,
        choices=[1, 2, 3, 4, 5, 6, 7],
        help="只运行指定步骤（1=models 2=responses 3=stream 4=tools 5=roundtrip 6=custom 7=web_search）",
    )
    args = parser.parse_args()

    key = resolve_key(args.key)
    print(f"{C_DIM}Base URL: {BASE_URL}{C_RESET}")

    model = "deepseek-chat"
    steps = [args.only] if args.only else [1, 2, 3, 4, 5, 6, 7]

    assistant_msg = None
    if 1 in steps:
        model = probe_models(key)
    if 2 in steps:
        probe_chat_nonstream(key, model)
    if 3 in steps:
        probe_chat_stream(key, model)
    if 4 in steps:
        assistant_msg = probe_tool_request(key, model)
    if 5 in steps:
        probe_tool_roundtrip(key, model, assistant_msg)
    if 6 in steps:
        probe_custom_apply_patch(key, model)
    if 7 in steps:
        probe_native_web_search(key, model)

    print(f"\n{C_GREEN}{C_BOLD}探测完成。{C_RESET}")


if __name__ == "__main__":
    main()
