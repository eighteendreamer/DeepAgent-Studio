# MCP Builder — FastMCP (Python) Skeleton

A minimal, task-oriented FastMCP server with strict schemas, recoverable errors,
and an evaluation outline.

## Server skeleton

```python
from fastmcp import FastMCP

mcp = FastMCP("weather")

@mcp.tool()
def get_forecast(city: str, days: int = 3) -> dict:
    """Return a short weather forecast for a city.

    Args:
        city: City name, e.g. "Seattle".
        days: Number of days (1-7). Defaults to 3.
    """
    if not city.strip():
        # Recoverable, model-readable error — not a stack trace.
        return {"error": "city must be a non-empty string"}
    if not 1 <= days <= 7:
        return {"error": "days must be between 1 and 7"}
    # ... call the external API, then return structured data ...
    return {"city": city, "days": days, "forecast": [...]}

if __name__ == "__main__":
    mcp.run()  # stdio transport by default
```

## Design rules

- Tools map to **tasks** ("get a forecast"), not raw endpoints.
- Parameters: clear names, types, descriptions, sane defaults; mark required vs
  optional explicitly.
- Returns: structured dicts the model can parse, not free-form prose.
- Errors: return `{"error": "..."}` with an actionable message; reserve
  exceptions for truly exceptional cases.
- Secrets: read from env/config; never echo them in tool output.

## Evaluation outline

Define realistic tasks and check the server enables them:

```python
EVALS = [
    {"task": "3-day forecast for Seattle", "call": {"city": "Seattle"},
     "check": lambda r: "forecast" in r and len(r["forecast"]) == 3},
    {"task": "reject empty city", "call": {"city": ""},
     "check": lambda r: "error" in r},
    {"task": "reject out-of-range days", "call": {"city": "Paris", "days": 99},
     "check": lambda r: "error" in r},
]
```

Iterate on tool naming, schemas, and defaults until every eval passes. The eval
suite — not API coverage — is the quality bar.
