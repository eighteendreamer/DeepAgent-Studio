# **Agent 意图识别与路由**

## **一、意图识别是什么**

### **1. 本质**

意图识别（Intent Recognition）就是：

**理解用户想做什么，并将自然语言转换成结构化任务，再决定后续应该走哪条处理链路。**

传统 NLP 更多关注：

```text
用户输入
  ↓
Intent Classification
  ↓
意图标签
```

例如：

```text
“帮我查一下昨天的订单”

→ query_order
```

但在 Agent 场景中，仅仅识别一个 Intent 通常不够，还需要进一步识别：

```text
用户输入
   ↓
Intent        → 用户想做什么
Slot          → 完成任务需要什么参数
Task          → 具体要执行什么任务
Route         → 应该走哪条链路
```

例如：

```text
“帮我查一下昨天北京的订单量”

Intent:
query_order

Slots:
date = 昨天
city = 北京
metric = 订单量

Route:
Order Agent
```

所以现代 Agent 中的意图识别可以理解成：

**Intent + Slot + Task + Routing 的统一入口。**

------

## **二、常见面试问题**

### **1. 意图识别到底用什么？**

常见方案：

```text
规则
小模型
大模型
```

核心问题是：

为什么不用一种方案，而要组合？

------

### **2. 规则、小模型、大模型怎么选择？**

| **方案** | **适合场景**                   |
| -------- | ------------------------------ |
| 规则     | 明确、高频、固定表达、特殊边界 |
| 小模型   | 稳定的闭集意图、高并发、低延迟 |
| LLM      | 复杂语义、多意图、歧义、长尾   |

生产环境通常不是三选一，而是：

```text
规则
 ↓
小模型
 ↓
LLM
```

逐级升级。

------

### **3. 意图识别出来了，但参数不完整怎么办？**

例如：

“帮我查一下订单。”

识别：

```text
Intent = query_order
```

但可能缺少：

```text
date
order_id
merchant_id
```

这时候涉及：

**Slot Filling + Clarification**

系统不能直接执行，而应该追问：

“你想查询哪个时间范围的订单？”

用户补充后，再更新当前 State，继续执行。

------

### **4. 一句话包含多个意图怎么办？**

这是 Agent 场景区别于传统单意图分类的重要问题。

例如：

“帮我查一下昨天订单量，然后分析退款率为什么上涨，最后把结果发到飞书群。”

实际上包含：

```text
Task 1：查询订单量
Task 2：分析退款率
Task 3：发送飞书
```

这时不能简单地：

```text
Intent
 ↓
Router
 ↓
一个 Agent
```

而需要进一步进行：

```text
Intent / Task Parse
       ↓
   Multi-Intent
       ↓
     Planner
       ↓
Task Decomposition
       ↓
Task Graph
```

例如：

```text
Task 1：查询订单量
       ↓
Task 2：分析退款率
       ↓
Task 3：生成总结并发送
```

如果任务之间没有依赖，也可以并行：

```text
             Planner
            /       \
           ↓         ↓
      Order Agent  Analytics Agent
           \         /
            ↓       ↓
             Aggregator
                  ↓
             Feishu Tool
```

所以这里需要注意：

**多意图不代表一定要把 Router 换成 Planner。**

更准确的架构是：

```text
Router
├── 单意图
│     ↓
│   直接路由
│
└── 多意图 / 复杂任务
      ↓
    Planner
      ↓
  任务拆解 + 编排
```

即：

**Router 负责入口分流，Planner 负责复杂任务的进一步拆解和编排。**

------

### **5. 单路由和多路由怎么处理？**

#### **单意图**

```text
User
 ↓
Router
 ↓
Order Agent
```

#### **多意图**

```text
             Router
                ↓
             Planner
                ↓
       ┌────────┼────────┐
       ↓        ↓        ↓
    Agent A   Agent B   Tool C
       └────────┼────────┘
                ↓
            Aggregator
                ↓
             Response
```

需要考虑：

- 串行还是并行
- Agent 之间是否存在依赖
- 一个 Agent 失败怎么办
- 最终结果怎么汇总

------

### **6. 意图识别后到底走 RAG、Tool 还是 Agent？**

意图识别最终解决的是：

**“这个请求应该交给谁处理？”**

例如：

```text
Chat
 ↓
LLM

知识问答
 ↓
RAG

订单查询
 ↓
API / SQL

日志排查
 ↓
Ops Agent

执行操作
 ↓
Tool
```

所以：

**RAG 不是所有请求都必须经过的环节，而只是 Router 的一种能力。**

------

### **7. 用户闲聊怎么办？**

例如：

“你好。”

“你是谁？”

这种请求一般：

```text
Intent = Chat
 ↓
LLM
```

不需要：

```text
Embedding
 ↓
Vector DB
 ↓
Reranker
```

因此系统需要明确的：

```text
Chat Intent
```

避免所有请求都进入 RAG 或业务 Agent。

------

### **8. 用户什么信息都没说清楚怎么办？**

例如：

“帮我看一下。”

此时甚至无法确定具体 Intent。

不能强行猜：

```text
intent = order_query
```

应该：

```text
低确定性
 ↓
Clarification
 ↓
用户补充
 ↓
重新识别
```

例如：

“你是想查询订单、退款情况，还是分析经营数据？”

------

### **9. LLM 判断的路由可靠吗？**

不能完全依赖。

例如 LLM 输出：

```json
{
  "intent": "refund",
  "route": "refund_agent"
}
```

程序还需要校验：

```text
Route 是否存在？
Slot 是否完整？
用户有没有权限？
是否属于高风险？
是否需要确认？
```

所以：

**LLM 负责理解和提出决策，程序负责约束和执行。**

------

### **10. 意图识别错了怎么办？**

必须考虑：

```text
低置信度
未知意图
意图冲突
模型判断错误
```

常见处理：

```text
高确定性
 ↓
直接路由

中确定性
 ↓
二次判断 / LLM

低确定性
 ↓
澄清 / Fallback

完全未知
 ↓
UNKNOWN / 通用对话 / 人工
```

------

### **11. 意图识别怎么评估？**

不能只看 Accuracy，还需要关注：

```text
Intent Precision / Recall / F1
Slot Accuracy
错误路由率
澄清率
Unknown / OOD 识别
任务成功率
延迟
Token Cost
```

真正生产环境最重要的是：

**用户的任务有没有被正确完成。**

------

## **三、生产环境应该怎么做？**

生产环境推荐采用：

**规则 + 小模型 + LLM + State + Router + Planner + Policy + Fallback**

整体流程：

```text
                         用户输入
                            ↓
                      Context / State
                            ↓
                    Intent + Slot 提取
                            ↓
              ┌─────────────┴─────────────┐
              │                           │
          规则能确定？                 不能确定
              │                           │
              ↓                        小模型
            Route                         ↓
                                      能确定？
                                     ┌────┴────┐
                                    Yes        No
                                     │          │
                                     │         LLM
                                     │          ↓
                                     └──────────┘
                                           ↓
                                  Intent + Slot + Task
                                           ↓
                                      Slot Validation
                                           ↓
                              ┌────────────┴────────────┐
                              │                         │
                           不完整                     完整
                              │                         │
                              ↓                         ↓
                          Clarification              Router
                              │                         ↓
                              └──────────────→──────────┤
                                                        ↓
                                            是否单意图 / 简单任务？
                                              ┌─────────┴─────────┐
                                             Yes                   No
                                              ↓                     ↓
                                         直接路由                Planner
                                              ↓                     ↓
                                           Agent              Task Decomposition
                                                                    ↓
                                                             Task Graph / Plan
                                                                    ↓
                                                        ┌───────────┼───────────┐
                                                        ↓           ↓           ↓
                                                      Agent A     Agent B     Tool C
                                                        └───────────┼───────────┘
                                                                    ↓
                                                               Aggregator
                                                                    ↓
                                                                  Policy
                                                                    ↓
                                                                Response
```

------

### **1. Intent + Slot：先把用户需求结构化**

例如：

```json
{
  "intent": "business_analysis",
  "slots": {
    "date": "昨天",
    "region": "北京"
  },
  "tasks": [
    "query_order",
    "analyze_refund"
  ]
}
```

不要让后续 Agent 直接从一段自然语言中自行猜测所有信息。

------

### **2. State：保存任务状态**

多轮对话：

```text
用户：帮我查订单
系统：哪个时间？
用户：昨天
系统：哪个地区？
用户：北京
```

State：

```json
{
  "intent": "query_order",
  "slots": {
    "date": "昨天",
    "region": "北京"
  },
  "status": "ready"
}
```

所以：

**Intent 不是一次性的分类，而是随着多轮对话不断更新 State。**

------

### **3. Router：负责入口分流**

Router 不一定自己完成整个任务。

主要负责判断：

```text
Chat
RAG
SQL
Tool
Single Agent
Planner
```

例如：

```text
query_order
   ↓
Order Agent

knowledge_qa
   ↓
RAG

log_analysis
   ↓
Ops Agent

复杂多意图
   ↓
Planner
```

------

### **4. Planner：只处理复杂、多意图任务**

这是和单路由架构最重要的区别。

简单任务：

```text
User
 ↓
Intent
 ↓
Router
 ↓
Agent
```

复杂任务：

```text
User
 ↓
Intent
 ↓
Router
 ↓
Planner
 ↓
Task Decomposition
 ↓
Task Graph
 ↓
多个 Agent / Tool
 ↓
Aggregator
```

Planner 需要解决：

```text
任务怎么拆？
哪些任务可以并行？
哪些任务存在依赖？
执行顺序是什么？
某个任务失败怎么办？
```

例如：

```text
Task A：查询订单
       ↓
Task B：查询退款率
       ↓
Task C：分析原因
       ↓
Task D：生成总结
       ↓
Task E：发送飞书
```

如果：

```text
A、B
```

互相独立，可以并行：

```text
        ┌→ Task A ─┐
Planner ┤          ├→ Task C → Task D → Task E
        └→ Task B ─┘
```

------

### **5. Aggregator：汇总多路结果**

多个 Agent 返回：

```text
Order Agent：
昨天订单量 12,530

Analytics Agent：
退款率 8.2%，上涨 2.1%
```

Aggregator 再统一生成最终结果：

```text
昨天北京订单量为 12,530 单，
退款率为 8.2%，较前一天上涨 2.1%。

结合订单和退款数据来看……
```

因此：

```text
Multi-Agent
    ↓
Results
    ↓
Aggregator
    ↓
Final Response
```

------

### **6. Policy：模型决策和程序执行之间增加约束**

即使 LLM 判断：

```text
route = delete_data
```

也不能直接执行。

需要程序校验：

```text
用户权限
参数合法性
风险等级
是否需要确认
是否允许当前 Agent 执行
```

最终：

```text
LLM
 ↓
Router / Planner
 ↓
Policy
 ↓
Allow / Confirm / Reject
 ↓
Execute
```

核心原则：

**识别出意图 ≠ 获得执行权限。**

------

### **7. Fallback：处理所有边界情况**

生产系统必须考虑：

```text
未知意图
低置信度
缺少 Slot
多意图冲突
LLM 输出错误
Tool 调用失败
Agent 超时
多个 Agent 结果冲突
```

例如：

```text
无法判断
 ↓
Clarification

完全未知
 ↓
UNKNOWN / Chat / 人工

Agent 失败
 ↓
Retry / Fallback

高风险操作
 ↓
Human Confirmation
```

------

## **四、最终总结**

整个生产方案可以浓缩成：

```text
                  用户输入
                     ↓
                  Intent
                     ↓
                   Slot
                     ↓
                  State
                     ↓
                  Router
                     ↓
          ┌──────────┴──────────┐
          ↓                     ↓
      单意图/简单任务        多意图/复杂任务
          ↓                     ↓
       直接路由              Planner
          ↓                     ↓
     Agent / RAG / Tool     Task Decomposition
                                ↓
                         多 Agent / Tool
                                ↓
                           Aggregator
                                ↓
                             Policy
                                ↓
                            Response
```

意图识别本身：

```text
规则 → 小模型 → LLM
```

复杂任务：

```text
Router → Planner → Task Graph → Agent / Tool → Aggregator
```

核心原则：

**简单问题规则解决，稳定问题小模型解决，复杂问题 LLM 解决；Router 负责分流，Planner 负责复杂任务编排，State 负责多轮上下文，Policy 负责约束，Fallback 负责兜底。**

最终面试真正要回答的不是：

“你们用了什么模型做 Intent Classification？”

而是：

**“如何把用户自然语言可靠地转换成可执行任务，并在缺参数、多意图、闲聊、低置信度和异常情况下，保证请求能够正确路由、执行和返回结果。”**