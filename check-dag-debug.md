# DAG 可视化调试清单

## 步骤 1：打开开发者工具

在 Desktop 应用中，按 `F12` 或 `Ctrl+Shift+I` 打开开发者工具。

## 步骤 2：查看 Console 输出

查找以下信息：

### 2.1 检查是否调用了 plan_execute
在 Console 中搜索：
- "plan_execute"
- "tool call"
- "ToolCall"

### 2.2 检查工具输出
查找工具返回的数据：
```javascript
// 应该看到类似这样的输出
{
  "report": "Plan executed...",
  "dag_execution": {
    "execution_id": "exec_...",
    "title": "...",
    "nodes": [...]
  }
}
```

## 步骤 3：检查网络请求

在 Network 标签页中：
1. 刷新对话
2. 查找包含 "plan_execute" 的请求
3. 检查响应内容

## 步骤 4：检查 React 组件渲染

在 Console 中输入：
```javascript
// 查找所有 plan_execute 工具调用
document.querySelectorAll('[class*="tool"]').forEach(el => {
  if (el.textContent.includes('plan_execute')) {
    console.log('Found plan_execute tool:', el);
    console.log('Classes:', el.className);
    console.log('HTML:', el.innerHTML);
  }
});
```

## 步骤 5：检查是否有 DagExecutionRow 组件

```javascript
// 查找 DAG 相关元素
document.querySelectorAll('[class*="dag"], [class*="phase"]').forEach(el => {
  console.log('Found DAG element:', el);
});
```

## 可能的问题

### 问题 1：Assistant 没有调用 plan_execute
**症状**：对话中只看到普通文本回复，没有工具调用

**原因**：
- 模型认为任务不需要多步骤编排
- 提示词不够明确

**解决**：尝试更明确的输入
```
Use plan_execute tool to implement authentication
```

### 问题 2：工具被调用但返回数据格式错误
**症状**：Console 中看到 "Failed to parse DAG execution data"

**原因**：
- 后端返回的 JSON 格式不正确
- 缺少 `dag_execution` 字段

**检查**：在 Console 中查看工具的原始输出

### 问题 3：前端解析失败
**症状**：工具调用存在，但显示为传统的 ProcessToolRow

**原因**：
- `parseDagExecution()` 返回 null
- 类型不匹配

**检查**：在 ProcessToolRow.tsx 中添加 console.log

### 问题 4：组件渲染但样式不正确
**症状**：看到 DAG 内容但布局混乱

**原因**：CSS 类名冲突或缺失

---

## 立即可做的检查

请在 Desktop 应用中：

1. **打开开发者工具** (F12)
2. **进入 Console 标签**
3. **粘贴并执行**：
```javascript
// 检查对话中的所有工具调用
const tools = document.querySelectorAll('[class*="tool"]');
console.log('Total tool calls found:', tools.length);

tools.forEach((tool, i) => {
  const text = tool.textContent;
  if (text.includes('plan') || text.includes('execute')) {
    console.log(`Tool ${i}:`, {
      text: text.substring(0, 100),
      classes: tool.className,
      element: tool
    });
  }
});
```

4. **发送结果**：把 Console 输出截图或复制给我

---

这样我们就能定位问题出在哪个环节了。
