---
name: reverse
description: Route authorized reverse engineering, JavaScript analysis, APK inspection, binary analysis, malware research, digital forensics, protocol analysis, firmware work, or security code audits to the appropriate bundled specialist workflow.
version: 1.1.0
---

# Reverse Engineering Router

This is the single entry point for the bundled `reverse-skill` pack. Keep its specialist modules lazy: do not scan or read every module up front.

1. Confirm the target, user intent, permitted files/systems, and whether active testing is authorized.
2. Read `resources/skills/MASTER-ROUTING.md`; use `resources/skills/routing.md` only when the primary route is ambiguous.
3. Read the selected module's `SKILL.md` completely before acting.
4. Prefer the current project's tools and runtimes. DeepAgent managed Node, Python, and JDK are compatibility fallbacks.
5. For JavaScript/browser work, prefer the built-in `js-reverse` MCP when its tools match the task.
6. For WeChat mini-program packages, use the built-in `wedecode` plugin and preserve the original package.
7. Keep evidence and generated output inside the current project or an explicitly authorized directory.

The bundled resources originate from `https://github.com/zhaoxuya520/reverse-skill` version 1.1.0 under the MIT license.
