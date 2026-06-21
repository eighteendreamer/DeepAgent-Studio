# Managed Runtime Licenses & Sources（office-agent）

托管运行时由 `RuntimeService` 按需下载到应用自有目录，**不随安装包分发**。它们以独立子进程 / 动态库方式被调用（非静态链接进本程序，属「聚合」），随附各自 license 与来源说明如下。分发这些二进制时，必须保留其许可证文本与版权声明。

| 运行时 | 用途 | 许可证 | 来源 |
|--------|------|--------|------|
| Whisper 模型 (ggml-base/small) | 语音转写模型 | MIT | https://huggingface.co/ggerganov/whisper.cpp |
| whisper.cpp（经 whisper-rs 静态链接） | 语音转写引擎 | MIT | https://github.com/ggerganov/whisper.cpp |
| Pandoc | Markdown↔docx 高保真转换 | GPL-2.0-or-later | https://github.com/jgm/pandoc |
| PDFium (bblanchon builds) | PDF 页面渲染 | BSD-3-Clause / Apache-2.0 | https://github.com/bblanchon/pdfium-binaries |
| LibreOffice | 旧格式转换 + 高保真 PDF 导出 | MPL-2.0 (LGPL-3.0 部分组件) | https://www.libreoffice.org / https://www.documentfoundation.org |

## 合规要点

- **GPL（pandoc）/ MPL·LGPL（LibreOffice）**：作为独立可执行子进程调用，不与本程序（Apache-2.0）静态链接。随分发须提供：
  - 各自完整许可证文本（安装时连同二进制写入其运行时目录）；
  - 来源 URL 与版本号（见上表 + `RuntimeService` 注册表条目）；
  - 对 GPL 组件，提供对应源码获取途径（上游仓库链接即可满足「书面要约」的实践做法）。
- **PDFium / Whisper（BSD/MIT/Apache）**：保留版权声明与许可证文本即可。
- 校验：每个 artifact 必须 pin SHA-256，下载后校验通过方可安装（fail-closed）。
- 安装位置：应用自有 `runtimes/` 目录（只读位置回退 app_data），不写系统目录 / PATH / 不需管理员，可一键卸载。

## 启用清单（pin 校验和时同步完成）

1. 为每个平台 artifact 填入实测 SHA-256（替换 `RuntimeService::default_registry` 中的 `sha256: None`）。
2. 将对应 LICENSE 文本随二进制一起落入运行时目录（或在首次安装时写入）。
3. 在「关于 / 第三方声明」界面列出本表内容。
