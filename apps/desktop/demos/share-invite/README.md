# Share & Invite 复刻预览

参考：[sucodee 原视频](https://x.com/sucodeee/status/2111061993599713612/video/1)（20.35 秒），以及 [Bluma 的 Share & Invite 在线预览](https://bluma.design/chat?lib=share)。

在 `apps/desktop` 目录运行：

```powershell
pnpm exec vite --config demos/share-invite/vite.config.ts
```

打开 <http://127.0.0.1:5174>。这是独立入口，不挂载到工作台，不调用 Tauri 或修改真实访问权限。

- 关闭后收拢成头像与 Share 按钮，再点 Share 展开。
- 输入邮箱，选择 Editor / Viewer；Enter、逗号或分号将邮箱加入待邀请列表，也支持一次粘贴多个邮箱。
- 点击 Send invite，邮箱变成标签，出现 Sending，随后添加到成员列表。
- Copy link 实际复制当前预览的 Q4 Plan 地址；复制失败会如实反馈。
- 刷新恢复初始三位成员。邀请为本地交互演示，不发送真实邮件。

复用项目已有的 Button、Input、Select / SelectTrigger / SelectContent / SelectItem；图标来自现有 lucide-react；动画来自现有 GSAP。没有新增依赖。

Inter 字体来自 [Google Fonts 官方仓库](https://github.com/google/fonts/tree/main/ofl/inter)，附带 `fonts/OFL.txt` 授权，随预览本地加载。

验证命令：

```powershell
pnpm exec tsc --project demos/share-invite/tsconfig.json
pnpm exec vite build --config demos/share-invite/vite.config.ts
```

已安装 Python Playwright 与 Chrome 时，保持预览服务运行，可执行 `python demos/share-invite/verify-motion.py` 验证真实逐帧运动。

尺寸、色彩、字体和弹簧节奏依据原视频画面重建。原站样式在隔离 iframe 内，未获取可直接比对的完整 CSS；动效时长为逐帧估算，不将该实现宣称为像素误差为零。

## 实际验证

2026-10-22：独立 TypeScript 检查与 Vite 生产构建通过；使用本机 Chrome 的无头模式跑过完整交互。375 / 768 / 1440 宽度均已截图自查，卡片为 320 × 378，无横向溢出。

已验证关闭与重新展开、隐藏内容不可聚焦、焦点恢复、权限选择、无效及重复邮箱、待邀请标签添加与移除、Sending 与成员加入、批量粘贴、真实剪贴板复制、Escape、减少动态效果设置和刷新复位。页面脚本错误为零。

保留参考的浅灰辅助文字；没有把这一配色宣称为通过完整无障碍审计。未运行桌面应用全量构建或 Rust 测试，本次未改桌面主入口、Tauri 或 Rust。

### 回弹校准

原录屏为可变帧率视频，本次按帧的实际时间戳提取白色外壳轮廓。展开时宽度领先高度约 57ms，高度峰值比静止状态约高 6%；收拢时高度先动，宽度约晚 70ms，再轻微回弹归位。数值为视频测量与拟合，不是原作者公开的物理参数。

`share-motion.ts` 保存分方向的阻尼曲线；GSAP 统一控制宽高、内容交错和取消。原来的第二个高度动画已移除，内容变化只重定向同一高度控制入口。按钮只在按下和松开时轻微压缩回弹，悬停不移动按钮或图标。
