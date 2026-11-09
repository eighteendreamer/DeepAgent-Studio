# Linux 升级架构图册 v2

从 [index.html](index.html) 打开离线图册。图示是未来设计，当前产品尚未接入 VM；具体源码改动、依赖与验收见[实施蓝图](../../linux-vm-implementation-blueprint.md)。

## 图源与发布规则

| 名称 | 图源 | 渲染产物 |
|---|---|---|
| 部署与信任边界 | [architecture.mmd](architecture.mmd) | [SVG](architecture.svg) |
| 源码组件改造 | [components.mmd](components.mmd) | [SVG](components.svg) |
| 命令完整时序 | [execution.mmd](execution.mmd) | [SVG](execution.svg) |
| 文件导入与提交 | [workspace.mmd](workspace.mmd) | [SVG](workspace.svg) |
| 存储与所有权 | [storage.mmd](storage.mmd) | [SVG](storage.svg) |
| 清理与恢复 | [maintenance.mmd](maintenance.mmd) | [SVG](maintenance.svg) |
| 启动与运行状态 | [lifecycle.mmd](lifecycle.mmd) | [SVG](lifecycle.svg) |

`.mmd` 是对应图的唯一可编辑逻辑源；`build.mjs` 产生 SVG 和离线 HTML，不手动修改派生产物。主方案中的 architecture/execution 内嵌源码必须与文件一致，`verify.py` 会检查。HTML 自带缩放、展开和图源链接，不加载 CDN、远程字体或第三方服务。

R/M/N/G 图例表达保留、扩展、新增、验证门槛。时序图展示的是未来交互契约，不表示已实现；guest/worker 的传输与进程能力仍需 UP-00 的实测。

## 重建与校验

文档校验不需要安装图形依赖，在仓库根运行：

```powershell
python docs/diagrams/linux-vm/verify.py
```

渲染需要已安装的 Mermaid 浏览器 bundle、Playwright 模块和 Chromium 系浏览器。`build.mjs` 不安装依赖、不改项目 lockfile、不联网。执行前在当前 shell 设置以下四个普通任务变量：`$diagramMermaid` 为 mermaid.min.js 的绝对路径，`$diagramPlaywright` 为 Playwright index.mjs 的绝对路径，`$diagramBrowser` 为 Chromium/Edge 可执行文件绝对路径，`$diagramEvidence` 为仓库外的截图/验证输出目录。

```powershell
node docs/diagrams/linux-vm/build.mjs --mermaid $diagramMermaid --playwright $diagramPlaywright --browser $diagramBrowser --evidence-dir $diagramEvidence
python docs/diagrams/linux-vm/verify.py
```

当前机器使用仓库已有 Mermaid **11.16.1** 与已安装 Playwright/Edge 完成重建；没有增加产品依赖。实际参数、版本、图尺寸和交互检查结果保存在[本次渲染记录](G:/Code_Warehouse/DeepAgent-VM-Lab/architecture-v2-validation/render-validation.json)；[文档校验记录](G:/Code_Warehouse/DeepAgent-VM-Lab/architecture-v2-validation/document-validation.json)包含文件链接、内嵌图一致性与预算校验。

## 本次完成检查

- [x] 以实际源码核查 E31～E39，将证据、缺口与 UP 改造单元关联。
- [x] 七张 Mermaid 语法解析和 SVG 渲染通过；图源与主方案内嵌图一致。
- [x] 在真实无头浏览器中打开离线图册，检查七个视图、缩放、展开及页面错误。
- [x] 检查 1600 和 900 像素宽度下的图册截图，并目视检查七张图。
- [x] 检查 UTF-8、代码围栏、本地链接、SVG、13 个 UP、9 个接口及 3,000 MB 预算。
- [x] 完成差异检查；本轮没有改产品代码，不将文档验证当作产品编译、VM 测试或跨平台兼容性验收。

上述勾选只针对本次文档交付。未来修改后需要重新执行相同检查，不是永久有效的产品测试记录。
