# 终端、归档与搜索

## 21. 归档、置顶与搜索

归档能力由 `ArchiveService` 提供：

- 单会话归档：`archive_conversation`
- 当前/项目批量归档：`archive_project_conversations`
- 全部归档：`archive_all_conversations`
- 列出归档：`list_archived_conversations`
- 恢复归档：`unarchive_conversation`
- 删除归档：`delete_archived_conversation`
- 删除全部归档：`delete_all_archived_conversations`

会话置顶由 `SessionStateService` 提供；项目置顶由 `ProjectService` 提供。前端搜索在 `SearchModal` 中完成，基于已加载 sessions/projects。

## 22. 终端与命令执行

`TerminalService` 在 active project 目录中执行命令，若没有 active project 则回退到默认 cwd。它包含危险命令拦截逻辑，空命令直接 no-op。

```mermaid
flowchart TD
    UI[TerminalPlugin] --> API[runTerminal(command)]
    API --> T[run_terminal]
    T --> TS[TerminalService::run]
    TS --> Cwd{active project?}
    Cwd -->|是| ProjectCwd[项目目录]
    Cwd -->|否| DefaultCwd[启动目录]
    ProjectCwd --> Guard{危险命令?}
    DefaultCwd --> Guard
    Guard -->|是| Block[返回 blocked result]
    Guard -->|否| Exec[执行命令并返回 stdout/stderr/code]
```

