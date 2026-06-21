# 实施计划：日常使用模式（daily-office-mode）

## Overview

本计划把"工作模式"开关（code / daily）从纯 UI 摆设变成贯穿前后端的真实行为：UI 切换 → 持久化到后端设置 → 每轮对话透传 work_mode → 系统提示按模式选人格 + skills 按模式加权。所有改动严格遵守两条硬约束：**不新增 crate**（仅改 `deepagent-app-core`、`deepagent-skills` 与前端）与 **code 路径字节不变**（daily 人格作为独立分支注入，code 模式系统提示与本特性之前逐字节一致，保住前缀缓存）。

任务按依赖顺序排列：先在 `deepagent-app-core` 建立 `WorkMode` 类型与持久化地基，再扩展 `deepagent-skills` 的分类与排序，然后接入系统提示与 ChatService 透传，最后打通 Tauri 命令与前端，并以集成测试与质量门禁收尾。每个任务都可独立编译与测试。

## Tasks

- [ ] 1. 在 deepagent-app-core 定义 WorkMode 枚举
  - 新增 `WorkMode` 枚举（`Code` / `Daily`），`#[derive(... Serialize, Deserialize, Default)]`、`#[serde(rename_all = "lowercase")]`、`#[default] Code`
  - 实现 `as_str()` 与 `parse(&str) -> WorkMode`（未知值容错回退 `Code`）
  - 放在新文件 `work_mode.rs` 或并入 settings 模块，并在 crate 内导出
  - _Requirements: 1.1, 2.4, 8.2_

  - [ ]* 1.1 为 WorkMode 编写单元测试
    - 测试 serde 序列化为 `"code"` / `"daily"`，默认反序列化为 `Code`
    - 测试 `parse` 对未知字符串回退 `Code`
    - _Requirements: 1.1, 2.4, 8.2_

- [ ] 2. 在 SettingsService 持久化 work_mode
  - [ ] 2.1 在 settings/app 设置结构体新增 work_mode 字段
    - 在 `settings.rs` 的 `AppSettings`（collection="settings" id="app"）新增 `#[serde(default)] pub work_mode: WorkMode`
    - 确保旧配置（无该字段）反序列化为 `Code`
    - _Requirements: 1.1, 8.2_

  - [ ] 2.2 实现 get_work_mode / set_work_mode
    - `get_work_mode(&self) -> WorkMode` 读取设置文档
    - `set_work_mode(&self, mode: WorkMode) -> Result<()>` 读改写设置文档，写失败返回 `Result` 错误
    - _Requirements: 1.2, 1.3, 1.4_

  - [ ]* 2.3 为设置往返编写单元测试
    - 测试 `set_work_mode(daily)` 后 `get_work_mode` 返回 `daily`
    - 测试旧配置（无 work_mode 字段）经 `get_work_mode` 得到 `Code`
    - _Requirements: 1.3, 1.4, 8.2_

- [ ] 3. 在 deepagent-skills 引入 SkillCategory 与 frontmatter 解析
  - [ ] 3.1 定义 SkillCategory 并加入 SkillMeta
    - 在 `skill.rs` 新增 `SkillCategory` 枚举（`Office` / `Coding` / `General`），`Serialize, Deserialize, Default`、`#[serde(rename_all = "lowercase")]`、`#[default] General`
    - `SkillMeta` 新增 `#[serde(default)] pub category: SkillCategory`
    - _Requirements: 4.1, 8.4_

  - [ ] 3.2 在 frontmatter 解析 category（含容错）
    - 在 `frontmatter` 解析逻辑读取 `category` 键并写入 `SkillMeta.category`
    - 缺省 → `General`；非法值（不在 office/coding/general）→ `General` 并记 `tracing::warn` 解析告警，且不中断 skill 加载
    - _Requirements: 4.2, 4.3, 4.4, 8.4_

  - [ ]* 3.3 为 SkillCategory 解析编写单元测试
    - 测试声明 office/coding/general 正确解析
    - 测试缺省 → general；非法值 → general 并产生告警
    - _Requirements: 4.2, 4.3, 4.4_

- [ ] 4. 在 deepagent-skills 实现按模式加权排序
  - [ ] 4.1 定义最小选择模式枚举并实现模式加权入口
    - 在 `deepagent-skills` 内定义排序所需最小枚举（如 `SkillSelectionMode { Office, Coding }`），避免反向依赖 app-core
    - 新增 `match_query_for_mode(&self, query: &str, mode: SkillSelectionMode) -> Vec<SkillMatch>`，保留原 `match_query` 不变
    - 加权：office 模式 office>coding（如 office×1.0、coding×0.5、general×0.8）；coding 模式 coding>office（coding×1.0、office×0.5、general×0.8）；general 两模式都为候选
    - 排序键 `(category_weight × trigger_score)` 降序，分相同时按原顺序稳定排序
    - _Requirements: 5.1, 5.2, 5.3, 6.2, 6.3_

  - [ ]* 4.2 为模式加权排序编写单元测试
    - 测试 daily/office 下 office 技能排在 coding 之上
    - 测试 code/coding 下 coding 技能排在 office 之上
    - 测试 general 技能在两种模式下均出现在候选中
    - _Requirements: 5.1, 5.2, 5.3_

- [ ] 5. 在 SkillsService 暴露 category 并接入模式排序
  - 在 `skills_service.rs` 的 `SkillDto` 新增 `category: SkillCategory` 字段并填充
  - 被动激活计算时接受 `work_mode`，将 app-core 的 `WorkMode` 映射为 `deepagent-skills` 的选择模式并调用 `match_query_for_mode`
  - 复用现有 skill 发现机制（office skill 即 category=office 的普通 skill），重新加载后能纳入新发现的 office skill
  - _Requirements: 4.5, 5.4, 6.1, 6.4_

  - [ ]* 5.1 为 SkillsService 模式接入编写单元测试
    - 测试 `SkillDto` 含 category 字段
    - 测试传入 daily 时被动激活结果按 office 优先排序
    - _Requirements: 4.5, 5.4_

- [ ] 6. 实现模式专属系统提示人设
  - [ ] 6.1 新增 DAILY_PERSONA_SEGMENT 并在 build_system_prompt 分支
    - 定义常量 `DAILY_PERSONA_SEGMENT`（面向办公、平实口语、少技术黑话、先结论后步骤）放在静态侧（Prompt_Boundary 之前）
    - `build_system_prompt(work_mode, ...)`：`Code` 走与现状逐字节相同的 `system_prompt_base()` 路径；`Daily` 在静态前缀末尾追加 `DAILY_PERSONA_SEGMENT`
    - code 分支不得因引入 mode 改动任何字节
    - _Requirements: 3.1, 3.2, 3.3, 3.5, 9.4_

  - [ ]* 6.2 为系统提示人设编写单元测试
    - 测试 code 输出与基线逐字节相同（快照/字节比对）
    - 测试 daily 输出包含办公人格段
    - 测试同一 mode 两次构建的静态前缀逐字节一致（前缀缓存）
    - _Requirements: 3.1, 3.2, 3.4, 9.4_

- [ ] 7. ChatService 透传 work_mode（方案 A 显式参数）
  - 给 `run` 与 `run_in_session` 增加 `work_mode: WorkMode` 参数
  - 内部把 work_mode 传给 `build_system_prompt(work_mode, ...)` 与 SkillsService 被动激活排序
  - 未提供时使用 `WorkMode::default()` = `Code`；每轮使用本轮提供的值
  - 更新 crate 内所有调用点编译通过
  - _Requirements: 2.1, 2.2, 2.3, 2.4, 2.5, 8.3_

  - [ ]* 7.1 为 ChatService 透传编写单元测试
    - 测试缺省 work_mode 时按 Code 运行
    - 测试两轮之间切换 work_mode 时使用各自本轮的值
    - _Requirements: 2.4, 2.5, 8.3_

- [ ] 8. 接入 Tauri 命令
  - 新增 `get_work_mode() -> WorkMode` 与 `set_work_mode(mode: WorkMode)` 命令，转发 SettingsService
  - 现有 chat 启动/继续对话命令增加 `work_mode: WorkMode` 入参并转发给 ChatService
  - 在命令注册表登记新命令
  - _Requirements: 1.2, 2.2, 2.3, 7.3_

- [ ] 9. 前端打通模式切换与透传
  - [ ] 9.1 在 api.ts / types.ts 增加封装与类型
    - 新增 `WorkMode` 类型与 `getWorkMode` / `setWorkMode` 封装
    - chat 调用增加 `work_mode` 入参
    - _Requirements: 1.5, 2.2, 2.3_

  - [ ] 9.2 StartView 模式切换写后端 + Mode_Badge + 发消息透传
    - 主界面模式切换调 `set_work_mode`（替代仅写 localStorage）
    - 显示 Mode_Badge（如 "Office Mode" / "编程模式"），切换即时更新、无需重启
    - 发消息时把当前 work_mode 透传给 chat 命令
    - _Requirements: 7.1, 7.2, 7.3, 7.4_

  - [ ] 9.3 GeneralSettings 与 OnboardingWizard 接入 set_work_mode 并状态同步
    - GeneralSettings 模式项调 `set_work_mode`
    - OnboardingWizard 选择模式后调 `set_work_mode` 持久化
    - work_mode 以后端设置为单一真相源，切换后各处读取同一值并刷新
    - _Requirements: 1.2, 1.5, 7.2_

- [ ] 10. 集成测试
  - [ ]* 10.1 端到端 daily 流程测试
    - `set_work_mode(daily)` → run 携带 daily → 系统提示含办公人格且 office skill 被动激活靠前
    - _Requirements: 3.1, 5.1, 6.2_

  - [ ]* 10.2 向后兼容测试
    - 未设模式的旧会话 run → code 行为不变（系统提示、工具、技能处理一致）
    - _Requirements: 8.1, 8.2, 8.3, 8.4_

- [ ] 11. 质量门禁检查点
  - 确认未新增任何 crate（仅改 deepagent-app-core / deepagent-skills / 前端）
  - 运行 `cargo clippy --workspace -- -D warnings` 无新增告警
  - 运行前端 `pnpm build` 通过
  - 运行全部单元与集成测试，确保通过；若有问题请询问用户
  - _Requirements: 9.1, 9.2, 9.3_

## Notes

- 标注 `*` 的子任务为可选（测试类），可为快速 MVP 跳过，但建议保留以满足需求 9.3。
- 每个任务都引用对应需求编号，便于追溯（需求 1-9）。
- 全程遵守两条硬约束：**不新增 crate** 与 **code 路径字节不变**（保前缀缓存与向后兼容）。
- 任务 6.2 通过字节快照比对守护 code 系统提示零改动，是向后兼容的关键防线。
- 检查点（任务 11）确保整体质量门禁达标。

## Task Dependency Graph

```json
{
  "waves": [
    { "id": 0, "tasks": ["1.1", "3.1"] },
    { "id": 1, "tasks": ["2.1", "3.2", "4.1"] },
    { "id": 2, "tasks": ["2.2", "3.3", "4.2", "6.1", "9.1"] },
    { "id": 3, "tasks": ["2.3", "5.1", "6.2", "7.1"] },
    { "id": 4, "tasks": ["8.1"] },
    { "id": 5, "tasks": ["9.2", "9.3"] },
    { "id": 6, "tasks": ["10.1", "10.2"] }
  ]
}
```
