# MCP 远程工具

## 18. MCP 远程工具

`McpService` 将 `.mcp.json` 风格配置存入文档存储，并为桌面设置页提供 CRUD。

MCP server DTO 字段：

- `name`
- `transport`: `stdio`、`sse`、`http`、`ws`
- `enabled`
- stdio 字段：`command`、`args`、`env`
- network 字段：`url`、`headers`

运行时连接流程：

```mermaid
flowchart TD
    Config[DocumentStore mcp/servers] --> Enabled[过滤 enabled servers]
    Enabled --> Expand[展开 ${ENV_VAR}]
    Expand --> Connect[connect_transport stdio/http/sse/ws]
    Connect --> Init[McpClient.initialize]
    Init --> List[McpClient.list_tools]
    List --> Registry[McpRegistry namespaced tools]
    Registry --> Adapter[McpToolAdapter]
    Adapter --> ToolRegistry[注册到运行时 ToolRegistry]
    Connect -->|失败| Failure[记录 failure，跳过该 server，不阻塞聊天]
```

命名空间规则由 `deepagent-mcp::registry` 提供，防止不同 server 的工具重名。

