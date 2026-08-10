<script setup>
import { ref } from 'vue'

const isDownloading = ref(false)
const downloadText = ref('软件下载 (Windows)')
const githubMirror = 'https://gh-proxy.com/'

function mirrorUrl(url) {
  return `${githubMirror}${url}`
}

async function downloadLatest() {
  if (isDownloading.value) return;
  isDownloading.value = true;
  downloadText.value = '获取最新版本...';
  try {
    // Fetch version from the raw main branch to completely bypass GitHub API rate limits (60/hr)
    const rawUrl = 'https://raw.githubusercontent.com/eighteendreamer/DeepAgent-Studio/main/apps/desktop/src-tauri/tauri.conf.json'
    const res = await fetch(mirrorUrl(rawUrl));
    if (!res.ok) throw new Error('Network Error');
    const tauriConf = await res.json();
    const version = tauriConf.version; // e.g., "0.0.4"
    
    if (version) {
      // Reconstruct the exact GitHub release download URL dynamically
      const githubUrl = `https://github.com/eighteendreamer/DeepAgent-Studio/releases/download/${version}/DeepAgent.Studio_${version}_x64-setup.exe`
      window.location.href = mirrorUrl(githubUrl)
    } else {
      throw new Error('Version not found in tauri.conf.json');
    }
  } catch (e) {
    console.warn('Fallback to releases page:', e);
    window.open('https://github.com/eighteendreamer/DeepAgent-Studio/releases/latest', '_blank');
  } finally {
    downloadText.value = '正在下载...';
    setTimeout(() => { downloadText.value = '软件下载 (Windows)'; isDownloading.value = false; }, 3000);
  }
}
</script>

<template>
  <main class="site-home">
    <section class="hero-stage">
      <div class="hero-noise"></div>
      <div class="hero-inner">
        <div class="hero-copy">
          <div class="eyebrow">
            <span class="status-dot"></span>
            DeepSeek Native Agent Runtime
          </div>
          <h1>DeepAgent Studio</h1>
          <p class="hero-lead">
            一个可验证、可回放、可扩展的 Agent 运行时平台，并在其上构建面向真实工作的桌面 IDE。
          </p>
          <div class="hero-actions">
            <a class="button primary" href="#" @click.prevent="downloadLatest" :class="{ 'opacity-80 cursor-wait': isDownloading }">{{ downloadText }}</a>
            <a class="button secondary" href="/core/architecture.html">查看系统架构</a>
            <a class="button secondary" href="/dev/workflow.html">开发者指南</a>
          </div>
        </div>

        <div class="product-scene" aria-label="DeepAgent Studio desktop preview">
          <div class="app-shell">
            <div class="app-topbar">
              <div class="window-controls">
                <span class="btn-close"></span>
                <span class="btn-min"></span>
                <span class="btn-max"></span>
              </div>
              <div class="topbar-title">DeepAgent Studio</div>
              <div class="topbar-right">
                <svg viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none"><path d="M12 20h9M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4L16.5 3.5z"></path></svg>
                <svg viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none"><circle cx="12" cy="12" r="3"></circle><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z"></path></svg>
              </div>
            </div>
            
            <div class="app-layout">
              <!-- Real Sidebar -->
              <aside class="app-sidebar">
                <div class="sidebar-top">
                  <div class="search-btn">
                    <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none"><circle cx="11" cy="11" r="8"></circle><line x1="21" y1="21" x2="16.65" y2="16.65"></line></svg>
                    Search...
                    <kbd>Ctrl K</kbd>
                  </div>
                  <div class="sidebar-actions">
                    <button class="new-chat-btn">
                      <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none"><line x1="12" y1="5" x2="12" y2="19"></line><line x1="5" y1="12" x2="19" y2="12"></line></svg>
                      New Chat
                    </button>
                    <button class="add-proj-btn">
                      <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path><line x1="12" y1="11" x2="12" y2="17"></line><line x1="9" y1="14" x2="15" y2="14"></line></svg>
                    </button>
                  </div>
                </div>
                
                <div class="sidebar-scroll">
                  <div class="side-group">
                    <div class="side-title">PROJECTS</div>
                    <div class="side-item active">
                      <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path></svg>
                      <span>DeepAgent Studio</span>
                    </div>
                    <div class="side-item">
                      <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path></svg>
                      <span>Office Agent</span>
                    </div>
                    <div class="side-item">
                      <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path></svg>
                      <span>CodeGraph Lab</span>
                    </div>
                  </div>
                  
                  <div class="side-group">
                    <div class="side-title">SESSIONS</div>
                    <div class="side-item">
                      <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none"><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"></path></svg>
                      <span>Runtime self-healing</span>
                    </div>
                    <div class="side-item">
                      <svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none"><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"></path></svg>
                      <span>MCP tool audit</span>
                    </div>
                  </div>
                </div>
                
                <div class="sidebar-bottom">
                  <div class="side-item"><svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none"><polygon points="12 2 2 7 12 12 22 7 12 2"></polygon><polyline points="2 17 12 22 22 17"></polyline><polyline points="2 12 12 17 22 12"></polyline></svg> Skills</div>
                  <div class="side-item"><svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none"><path d="M4 19.5A2.5 2.5 0 0 1 6.5 17H20"></path><path d="M6.5 2H20v20H6.5A2.5 2.5 0 0 1 4 19.5v-15A2.5 2.5 0 0 1 6.5 2z"></path></svg> Knowledge</div>
                  <div class="side-item"><svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none"><path d="M21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16z"></path></svg> Plugins</div>
                  <div class="side-item"><svg viewBox="0 0 24 24" width="14" height="14" stroke="currentColor" stroke-width="2" fill="none"><rect x="3" y="3" width="18" height="18" rx="2" ry="2"></rect><line x1="3" y1="9" x2="21" y2="9"></line><line x1="9" y1="21" x2="9" y2="9"></line></svg> Automation</div>
                </div>
              </aside>

              <!-- Real ChatView -->
              <section class="app-main">
                <div class="chat-scroll">
                  
                  <div class="chat-message human">
                    <div class="msg-avatar user">U</div>
                    <div class="msg-content">
                      <div class="text-block">重构一下桌面端的整个架构，先给出计划，再执行修改。</div>
                    </div>
                  </div>
                  
                  <div class="chat-message ai">
                    <div class="msg-avatar agent"><img src="/logo.png" alt="Agent" /></div>
                    <div class="msg-content">
                      <div class="agent-name">DeepAgent</div>
                      
                      <!-- Tool Call Card (real structure) -->
                      <div class="tool-call-card">
                        <div class="tool-head">
                          <div class="tool-title">
                            <svg viewBox="0 0 24 24" width="12" height="12" stroke="currentColor" stroke-width="2" fill="none"><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"></path><polyline points="15 3 21 3 21 9"></polyline><line x1="10" y1="14" x2="21" y2="3"></line></svg>
                            codegraph_impact
                          </div>
                          <div class="tool-meta">128ms</div>
                        </div>
                        <div class="tool-body">
                          target: RuntimeEngine::execute_tools<br/>
                          direct: 7 · indirect: 34 · status: replayable
                        </div>
                      </div>

                      <div class="text-block">
                        根据代码图谱分析，已为您生成可审计的架构调整计划。完成修改后，我会运行验证循环，如遇报错会自动捕获 observation 并尝试自我修复。
                      </div>
                    </div>
                  </div>
                  
                </div>

                <!-- Composer -->
                <div class="composer-container">
                  <div class="git-branch-chip">
                    <svg viewBox="0 0 24 24" width="12" height="12" stroke="currentColor" stroke-width="2" fill="none"><circle cx="18" cy="18" r="3"></circle><circle cx="6" cy="6" r="3"></circle><path d="M13 6h3a2 2 0 0 1 2 2v7"></path><line x1="6" y1="9" x2="6" y2="21"></line></svg>
                    main*
                  </div>
                  <div class="composer-box">
                    <button class="comp-btn attach">
                      <svg viewBox="0 0 24 24" width="18" height="18" stroke="currentColor" stroke-width="2" fill="none"><path d="M21.44 11.05l-9.19 9.19a6 6 0 0 1-8.49-8.49l9.19-9.19a4 4 0 0 1 5.66 5.66l-9.2 9.19a2 2 0 0 1-2.83-2.83l8.49-8.48"></path></svg>
                    </button>
                    <div class="comp-input">
                      Type a message or / for commands...
                    </div>
                    <button class="comp-btn mic">
                      <svg viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none"><path d="M12 1a3 3 0 0 0-3 3v8a3 3 0 0 0 6 0V4a3 3 0 0 0-3-3z"></path><path d="M19 10v2a7 7 0 0 1-14 0v-2"></path><line x1="12" y1="19" x2="12" y2="23"></line><line x1="8" y1="23" x2="16" y2="23"></line></svg>
                    </button>
                    <button class="comp-btn send">
                      <svg viewBox="0 0 24 24" width="16" height="16" stroke="currentColor" stroke-width="2" fill="none" transform="rotate(45)"><line x1="22" y1="2" x2="11" y2="13"></line><polygon points="22 2 15 22 11 13 2 9 22 2"></polygon></svg>
                    </button>
                  </div>
                </div>
              </section>
            </div>
          </div>
        </div>
      </div>
    </section>

    <section class="proof-strip">
      <div><strong>24</strong><span>Rust workspace crates</span></div>
      <div><strong>100+</strong><span>Tauri command surfaces</span></div>
      <div><strong>SQLite</strong><span>append-only event store</span></div>
      <div><strong>MCP</strong><span>live remote tool registry</span></div>
    </section>

    <section class="section-wrap">
      <div class="section-heading">
        <span class="eyebrow dark">Runtime Kernel</span>
        <h2>不是聊天 UI，而是 Agent 运行时内核。</h2>
        <p>DeepAgent Studio 将模型、工具、权限、记忆、技能和桌面能力压进一个可回放的运行闭环。</p>
      </div>

      <div class="feature-grid">
        <article>
          <span class="feature-no">01</span>
          <h3>事件溯源会话</h3>
          <p>消息、工具调用、reasoning、usage 和状态迁移全部写入事件流，历史会话可从 SQLite 重建。</p>
        </article>
        <article>
          <span class="feature-no">02</span>
          <h3>可审计工具执行</h3>
          <p>文件、bash、web、knowledge、Office、Git、MCP 与 codegraph 工具统一进入 ToolRegistry。</p>
        </article>
        <article>
          <span class="feature-no">03</span>
          <h3>权限与审批门控</h3>
          <p>沙箱模式、permission rules、Hook 生命周期与 UI 审批队列共同保护高风险动作。</p>
        </article>
        <article>
          <span class="feature-no">04</span>
          <h3>自愈验证循环</h3>
          <p>Agent 完成后触发验证计划，失败时反思、重试，直到通过、取消或达到循环上限。</p>
        </article>
      </div>
    </section>

    <section class="black-panel">
      <div class="panel-copy">
        <span class="eyebrow">System Flow</span>
        <h2>THINK、EXECUTE、OBSERVE，每一步都可追踪。</h2>
        <p>运行时将模型输出归约为完成、审批或工具调用，再把每次 observation 送回下一轮思考。</p>
      </div>
      <div class="flow-board">
        <div class="flow-node">Prompt Gate</div>
        <div class="flow-arrow"></div>
        <div class="flow-node">ModelAgent</div>
        <div class="flow-arrow"></div>
        <div class="flow-node">ToolRegistry</div>
        <div class="flow-arrow"></div>
        <div class="flow-node">EventStore</div>
      </div>
      <div class="terminal-window">
        <div class="terminal-top"><span></span><span></span><span></span></div>
        <pre>RuntimeEvent::SessionRegistered
RuntimeEvent::ReasoningDelta
RuntimeEvent::ToolStarted { name: "project_map_impact" }
RuntimeEvent::ToolCompleted { status: "ok" }
EventPayload::UsageRecorded</pre>
      </div>
    </section>

    <section class="split-showcase">
      <div class="showcase-copy">
        <span class="eyebrow dark">Desktop IDE</span>
        <h2>围绕真实工作流组织，而不是围绕一次对话。</h2>
        <p>
          多项目侧边栏、技能市场、知识库图谱、Git Workbench、项目地图、终端、文件预览、录音和 Office 文档工具被收束进一个桌面工作台。
        </p>
        <a class="text-link" href="/desktop/app-structure.html">浏览完整功能矩阵</a>
      </div>
      <div class="capability-stack">
        <div class="cap-row"><b>01</b><span>多项目会话归属</span><em>ProjectService</em></div>
        <div class="cap-row"><b>02</b><span>技能市场 + AI 安全审查</span><em>SkillsService</em></div>
        <div class="cap-row"><b>03</b><span>MCP 可视化配置</span><em>McpService</em></div>
        <div class="cap-row"><b>04</b><span>原生代码图谱</span><em>CodeGraph</em></div>
        <div class="cap-row"><b>05</b><span>Office / Speech / Recording</span><em>Office Agent</em></div>
      </div>
    </section>

    <section class="architecture-strip">
      <div class="section-heading compact">
        <span class="eyebrow dark">Architecture</span>
        <h2>Rust 内核与 Tauri 桌面壳的清晰分层。</h2>
      </div>
      <div class="asset-frame">
        <img src="/assets/system-architecture.svg" alt="DeepAgent Studio system architecture" />
      </div>
    </section>

    <section class="launch-section">
      <div>
        <span class="eyebrow dark">First Version</span>
        <h2>为开发者、团队和重度 Agent 用户准备。</h2>
        <p>从一个可运行的桌面端开始，逐步扩展到技能生态、MCP 工具网络、项目知识库和办公自动化。</p>
      </div>
      <a class="button primary inverse" href="/dev/workflow.html">开始集成</a>
    </section>
  </main>
</template>
