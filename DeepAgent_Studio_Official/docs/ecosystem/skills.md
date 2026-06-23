# 技能系统

## 16. 技能系统

技能由 `SKILL.md` 描述，可来自四层 roots：

1. BuiltIn：打包随应用发布的内置技能。
2. Installed/Marketplace：用户从市场安装的技能。
3. User：用户个人技能目录。
4. Workspace：项目内 `.deepagent/skills`。

优先级从低到高加载，后加载的同 id 技能覆盖前者：BuiltIn -> Installed -> User -> Workspace。

### 16.1 技能生命周期

```mermaid
stateDiagram-v2
    [*] --> Discovered: 扫描 SKILL.md
    Discovered --> Listed: SkillsView list
    Listed --> Previewed: preview_skill_activation(query)
    Listed --> Activated: activate_skill(id) 或模型调用 skill 工具
    Activated --> Injected: Level-2 body 注入 prompt
    Listed --> Uninstalled: uninstall_skill(id)
    Discovered --> Reloaded: reload_skills
```

### 16.2 技能市场安装流程

```mermaid
sequenceDiagram
    participant UI as SkillsView / SkillInstallDialog
    participant T as Tauri skill_market_*
    participant MP as SkillsMpClient
    participant Scan as Static scanner
    participant AI as AI security review
    participant SS as SkillsService
    participant CS as ChatService

    UI->>T: skill_market_search(query)
    T->>MP: search
    MP-->>UI: MarketSearchData
    UI->>T: skill_market_scan(githubUrl)
    T->>MP: download to temp
    T->>Scan: scan_dir(temp)
    T-->>UI: temp_id + ScanReport
    opt 设置启用 AI 审查
        UI->>T: skill_market_ai_review(temp_id)
        T->>AI: stream review tokens
        AI-->>UI: skill-ai-review token/done events
    end
    UI->>T: skill_market_install(temp_id)
    T->>SS: install_from_temp
    T->>CS: reset_all_sent_skills
    T-->>UI: SkillDto
```

安全细节：

- `PendingScan` 持有临时目录句柄，安装或取消前不会被提前删除。
- 临时扫描 TTL 为 30 分钟，相关命令会懒清理过期项。
- 安装前静态扫描风险，如 shell、网络、凭证、越界写、混淆等。
- 可选 AI 审查使用固定中文 system prompt，要求输出 `=== ANALYSIS ===` 和 `=== VERDICT ===`。
- 内置技能禁止卸载。
- 技能集发生变化后会重置 `ChatService` 的技能目录发送状态，下一轮重新提示模型。

