---
name: MCP Builder
description: This skill should be used when the user wants to "build an MCP server", "create a Model Context Protocol server", "define MCP tools", "integrate an external API as MCP", or "write MCP evaluations". Guide for creating high-quality MCP servers that let LLMs interact with external services through well-designed tools, in Python (FastMCP) or Node/TypeScript (MCP SDK).
version: 0.1.0
---

# MCP Builder

Create high-quality MCP (Model Context Protocol) servers that enable LLMs to
interact with external services through well-designed tools. The quality of an
MCP server is measured by how well it lets an LLM accomplish real-world tasks —
not by raw API coverage.

## When to use

Use when wrapping an external API or service as an MCP server (Python via
FastMCP, or Node/TypeScript via the MCP SDK), defining its tools, and writing an
evaluation suite to measure tool quality.

## Principles

1. **Design tools for tasks, not endpoints** — expose what an agent needs to
   accomplish a goal, with sensible defaults, not a 1:1 mirror of the API.
2. **Clear schemas** — precise parameter names, types, descriptions, and
   required/optional flags; return structured, model-readable results.
3. **Helpful errors** — return actionable error messages the model can recover
   from, not raw stack traces.
4. **Safety** — gate destructive operations; never leak secrets in tool output.

## Workflow

1. Choose a stack (Python FastMCP or Node/TS MCP SDK).
2. Model the *tasks* the agent must perform; derive the tool set from those.
3. Implement tools with strict input schemas and structured outputs.
4. Add auth (e.g. OAuth/token) and configuration.
5. Write an **evaluation suite**: realistic tasks the server should enable, with
   pass/fail checks — iterate on tool design until evals pass.
6. Register with `mcp__<server>__<tool>` namespacing on the client side.

## Bundled resources

- `references/fastmcp.md` — a FastMCP (Python) server skeleton with a tool,
  error handling, and an evaluation outline.
