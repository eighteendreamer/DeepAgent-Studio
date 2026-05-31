#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""DeepSeek API 协议探测脚本。

调用真实 DeepSeek API，把完整的请求 / 响应原样打印出来，用来确认：
  1. /models           —— 账号能用哪些模型
  2. chat（非流式）     —— 完整响应 JSON 的字段（含 reasoning_content）
  3. chat（流式 SSE）   —— 每个 data: chunk 的形态（delta / tool_calls 分片）
  4. function calling   —— DeepSeek 返回的 tool_calls **精确结构**
  5. tool_calls 回传    —— 把带 tool_calls 的 assistant 消息 + tool 结果消息发回去，
                           验证 DeepSeek 要求的精确 wire 格式（这步直接对应 400 报错）

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
    hr("2. POST /chat/completions（非流式）—— 完整响应 JSON")
    payload = {
        "model": model,
        "messages": [
            {"role": "system", "content": "你是一个简洁的助手。"},
            {"role": "user", "content": "用一句话介绍你自己。"},
        ],
        "stream": False,
    }
    pretty("request", payload)
    try:
        with post("/chat/completions", key, payload) as resp:
            body = json.loads(resp.read().decode("utf-8"))
        pretty("response", body)
        msg = body["choices"][0]["message"]
        print(f"{C_GREEN}message 字段: {list(msg.keys())}{C_RESET}")
        if "reasoning_content" in msg:
            print(f"{C_GREEN}存在 reasoning_content 字段{C_RESET}")
    except urllib.error.HTTPError as exc:
        read_error(exc)


def probe_chat_stream(key: str, model: str) -> None:
    hr("3. POST /chat/completions（流式 SSE）—— 每个 chunk 形态")
    payload = {
        "model": model,
        "messages": [{"role": "user", "content": "从 1 数到 3。"}],
        "stream": True,
    }
    pretty("request", payload)
    try:
        with post("/chat/completions", key, payload, stream=True) as resp:
            count = 0
            for raw in resp:
                line = raw.decode("utf-8", errors="replace").rstrip("\n")
                if not line.strip():
                    continue
                if not line.startswith("data:"):
                    print(f"{C_YELLOW}非 data 行: {line}{C_RESET}")
                    continue
                data = line[len("data:"):].strip()
                if data == "[DONE]":
                    print(f"{C_DIM}[DONE]{C_RESET}")
                    break
                count += 1
                if count <= 8:  # 只详细打印前几个
                    try:
                        pretty(f"chunk #{count}", json.loads(data))
                    except Exception:  # noqa: BLE001
                        print(f"chunk #{count}: {data}")
            print(f"{C_GREEN}共收到 {count} 个 data chunk{C_RESET}")
    except urllib.error.HTTPError as exc:
        read_error(exc)


def _weather_tool() -> dict:
    return {
        "type": "function",
        "function": {
            "name": "get_weather",
            "description": "查询某地天气",
            "parameters": {
                "type": "object",
                "properties": {"city": {"type": "string", "description": "城市名"}},
                "required": ["city"],
            },
        },
    }


def probe_tool_request(key: str, model: str):
    hr("4. function calling —— DeepSeek 返回的 tool_calls 精确结构")
    payload = {
        "model": model,
        "messages": [{"role": "user", "content": "北京今天天气怎么样？"}],
        "tools": [_weather_tool()],
        "stream": False,
    }
    pretty("request", payload)
    try:
        with post("/chat/completions", key, payload) as resp:
            body = json.loads(resp.read().decode("utf-8"))
        pretty("response", body)
        msg = body["choices"][0]["message"]
        calls = msg.get("tool_calls")
        if calls:
            print(f"{C_GREEN}{C_BOLD}DeepSeek 返回的 tool_call 结构（注意 type/function 嵌套）:{C_RESET}")
            pretty("tool_calls[0]", calls[0])
            print(
                f"{C_YELLOW}=> 顶层字段: {list(calls[0].keys())}  "
                f"function 字段: {list(calls[0].get('function', {}).keys())}{C_RESET}"
            )
        return msg
    except urllib.error.HTTPError as exc:
        read_error(exc)
        return None


def probe_tool_roundtrip(key: str, model: str, assistant_msg: dict | None) -> None:
    hr("5. tool_calls 回传 —— 验证 400 报错的精确 wire 格式")
    # 若上一步没拿到，就手工构造一个标准 assistant tool_calls 消息
    if not assistant_msg or not assistant_msg.get("tool_calls"):
        print(f"{C_DIM}（无上一步结果，使用标准构造的 assistant 消息）{C_RESET}")
        assistant_msg = {
            "role": "assistant",
            "content": "",
            "tool_calls": [
                {
                    "id": "call_demo_1",
                    "type": "function",
                    "function": {
                        "name": "get_weather",
                        "arguments": '{"city": "北京"}',
                    },
                }
            ],
        }
    call_id = assistant_msg["tool_calls"][0]["id"]

    # --- 5a. 正确格式：tool_call 含 type + function{name, arguments(字符串)} ---
    print(f"\n{C_BOLD}5a. 正确格式（含 type:function）{C_RESET}")
    ok_payload = {
        "model": model,
        "messages": [
            {"role": "user", "content": "北京今天天气怎么样？"},
            assistant_msg,
            {
                "role": "tool",
                "tool_call_id": call_id,
                "content": '{"city": "北京", "weather": "晴", "temp": 25}',
            },
        ],
        "tools": [_weather_tool()],
        "stream": False,
    }
    pretty("request", ok_payload)
    try:
        with post("/chat/completions", key, ok_payload) as resp:
            body = json.loads(resp.read().decode("utf-8"))
        print(f"{C_GREEN}成功！最终回复:{C_RESET}")
        print(body["choices"][0]["message"].get("content"))
    except urllib.error.HTTPError as exc:
        read_error(exc)

    # --- 5b. 错误格式：扁平 tool_call {id, name, arguments(对象)}（复现 400）---
    print(f"\n{C_BOLD}5b. 错误格式（扁平，缺 type/function）—— 复现我们当前的 400{C_RESET}")
    bad_payload = {
        "model": model,
        "messages": [
            {"role": "user", "content": "北京今天天气怎么样？"},
            {
                "role": "assistant",
                "content": "",
                "tool_calls": [
                    {
                        "id": call_id,
                        "name": "get_weather",
                        "arguments": {"city": "北京"},
                    }
                ],
            },
            {
                "role": "tool",
                "tool_call_id": call_id,
                "content": '{"weather": "晴"}',
            },
        ],
        "tools": [_weather_tool()],
        "stream": False,
    }
    pretty("request", bad_payload)
    try:
        with post("/chat/completions", key, bad_payload) as resp:
            body = json.loads(resp.read().decode("utf-8"))
        print(f"{C_YELLOW}意外成功了？{C_RESET}")
        print(body["choices"][0]["message"].get("content"))
    except urllib.error.HTTPError as exc:
        print(f"{C_GREEN}如预期失败（这正是我们要修的 bug）:{C_RESET}")
        read_error(exc)


def main() -> None:
    parser = argparse.ArgumentParser(description="DeepSeek API 协议探测")
    parser.add_argument("--key", help="DeepSeek API Key（sk-...）")
    parser.add_argument(
        "--only",
        type=int,
        choices=[1, 2, 3, 4, 5],
        help="只运行指定步骤（1=models 2=chat 3=stream 4=tools 5=roundtrip）",
    )
    args = parser.parse_args()

    key = resolve_key(args.key)
    print(f"{C_DIM}Base URL: {BASE_URL}{C_RESET}")

    model = "deepseek-chat"
    steps = [args.only] if args.only else [1, 2, 3, 4, 5]

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

    print(f"\n{C_GREEN}{C_BOLD}探测完成。{C_RESET}")


if __name__ == "__main__":
    main()
