# Harness 问题修复进度

## 当前状态与计数口径（2026-10-26）

本任务是已授权的本地项目修改与测试，不对第三方开展测试，不访问真实内网资源、真实凭据或用户数据。依据 [原审计报告](system-harness-audit-2026-10-21.md) 逐项修复，每轮只处理一个独立根因；H-78已完成所列Windows本机范围验收，累计十二项已验收，正准备独立提交。审计时已修复有验收的条目跳过；归并、撤销和补充证据不重复计数。

本台账按 **H-01～H-306 的 306 个历史编号槽位一一对应**，不是 306 个有效缺陷。原文有 299 个不同编号的主/归档标题；7 个没有独立主标题的旧编号从原报告归并说明及 Git 撤回记录补齐，仍各占一个追溯行，不新增有效任务。同一 H 的补充证据、真实进程补强或复核章节共用一行。表中标题直接来自当前原报告；无独立标题的旧编号用带“重复/撤销/归并”说明的追溯标题。原文链接指向同一报告，按 H 编号定位对应章节；旧编号按“归并目标/跳过依据”定位。

| 状态 | 数量 | 含义 |
|---|---|---|
| 本阶段已修复并验收 | 12 | H-286、H-72、H-73、H-75、H-76、H-20、H-22、H-23、H-33、H-32、H-44、H-78，均限所列本机验收；H-78待本次独立提交，其余hash见明细 |
| 历史已修复跳过 | 17 | 原报告有实现及修复后验收；本轮不重复修复/冒称重验 |
| 归并 | 7 | 只在唯一有效项修复，不增加完成数 |
| 撤销 | 2 | H-263/H-305，不作为缺陷或修复成果 |
| 修复中 | 0 | H-78 Windows本机验收及跨卷/Unix兼容结果见E-63～72；Unix操作期竞态、macOS/断电等未验证范围仍明示保留 |
| 待修复 | 268 | 执行前仍要核对当前代码、触发条件和原始测试；不是全部条目本轮重新验证未修 |
| 阻塞待处理 | 0 | 整套验证的环境/基线失败另列，不能据此假称通过 |

有效未关闭条目当前为 **268（268待修复）**；不提供完成率。十二项的本机范围验收不表示整个系统、全部平台或完整 workspace 已通过。历史修复依原报告状态跳过，但不虚构其 commit ID：尚未逐项重新定位的历史 hash 明确留为“未重新定位”，保留原文验收来源。

## H-78 本轮即时记录（2026-10-26）

当前状态：已修复并验收（Windows本机范围），待独立提交及hash回填；最终验收矩阵和限制见E-72，以下保留即时过程记录。范围仅本仓库文件工具物理路径授权，合成临时文件/本机联接与离线transport；不访问真实用户文件、内网、凭据或供应商，不重做H-165及加密内容报错。复用既有WorkspaceRoot/PathGuard/文件工具边界，不新增工具注册/权限中心。

E-H78-20261026-01（source_type=command，content_hash/artifact_path=n/a）：HEAD `8d8260e0`执行`cargo test -p deepagent-app-core --test kernel_v2_e2e path_boundary --offline -j 1 -- --include-ignored --nocapture --test-threads=1`退出1，1 passed/1 failed/0 ignored（0.73s，编译1m02s）。直接外部路径WorkspaceWrite拒绝/FullAccess允许的对照通过；真实临时NTFS junction组仍为`content_returned=true, tool_completed_ok=Some(true), tool_blocked=false`。F-H78（P1，validated，confidence=high，evidence_ids=[E-H78-20261026-01]）：共享WorkspaceRoot/PathGuard仅词法授权，生产读路径把外部合成canary带入离线下一wire。P-H78（callflow）：正式ChatService/Kernel → path_guard → WorkspaceRoot词法通过 → 文件系统跟随junction → function_call_output/下一请求；未读取用户数据、凭据或调用真实供应商。H-279仅归并证据，不重复计数；本轮尚未改生产代码，需共用物理路径/实际I/O边界，覆盖读写与隐式遍历，保留Full/ReadAnywhere原语义，其他独立问题不顺手关闭。

E-H78-20261026-02（句柄方案反证，非修复验收）：`cargo test -p deepagent-builtins --lib directory_identity_pin --offline -j 1 -- --nocapture`首次退出1，0 passed/2 failed/0 ignored（编译34.77s、测试0.04s）：`access_mode(0)`配合不共享写/删并未阻止真实普通目录和junction重命名。改为`read(true)`后同命令退出1，1 passed/1 failed/0 ignored（编译14.25s、测试0.04s）：目录/junction身份保护断言成立，但普通目录的子文件rename发布报Windows错误32。这是本轮候选机制的真实失败，不是已有产品回归失败；未将该句柄机制接入生产。微软[CreateFileW](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew)说明属性访问不受共享标志影响、共享限制持续到句柄关闭；不能从参数名称推断零访问句柄能pin身份，也不能以阻止全部写入充当正常文件工具修复。继续核对操作级边界，H-78不关闭。

E-H78-20261026-03（候选机制辨别）：第三轮`cargo test -p deepagent-builtins --lib directory_identity_pin --offline -j 1 -- --nocapture`退出0，3 passed/0 ignored（编译14.59s、测试0.05s）。此轮将已证会阻止子发布的模式明确作为反例特征测试保留，而非放宽生产行为要求；另验证`FILE_TRAVERSE|FILE_READ_ATTRIBUTES`配合共享READ/WRITE可阻止父目录rename并允许子文件发布，但仍能取得数据写句柄，不能单独证明防止原地reparse转换。补读Codex的`exec-server/src/no_follow/windows.rs`、`windows-sandbox-rs/src/no_reparse_dir.rs`和`file_write.rs`，其机制是实际open时OBJ_DONT_REPARSE、必要时目录guard及父句柄相对提交，不是仅靠canonicalize或零访问pin。微软[OBJECT_ATTRIBUTES](https://learn.microsoft.com/en-us/windows/win32/api/ntdef/ns-ntdef-_object_attributes)与[NtCreateFile](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/nf-ntifs-ntcreatefile)已核对；未执行参考项目测试。

E-H78-20261026-04（首阶段实现，非整项验收）：WorkspaceRoot共享物理预检检查最终目标及根内各前缀，缺失尾部使用最深已存在祖先、悬空链接报错，返回原路径保留cache/display/条目语义；read_file在H-76路径gate后再次校验，并通过既有windows-sys的NtCreateFile/OBJ_DONT_REPARSE打开已授权本地磁盘目标，从同一File读取正文和metadata。未复制参考业务代码、未新增依赖版本或lockfile变更，仅启用既有windows-sys所需features。`cargo test -p deepagent-builtins --lib fs_guard --offline -j 1 -- --nocapture`14 passed/0 ignored（0.15s）；真实内向junction、根alias、外部ReadAnywhere/Full对照、新建父路径、悬空/先外后内前缀及确定性“预检后替换父目录为junction”通过。`cargo test -p deepagent-builtins --offline -j 1 -- --quiet --test-threads=4`退出0，319 passed/1 ignored（25.86s），doc-tests0；ignored不计通过。`cargo test -p deepagent-app-core --test kernel_v2_e2e path_boundary --offline -j 1 -- --nocapture --test-threads=1`退出0，3 passed/0 ignored（编译2m03s、测试1.42s）：原探针转默认，实际`content_returned=false, tool_completed_ok=None, tool_blocked=true`；断言同步为与原直接拒绝对照相同的中央硬拒绝，额外外部junction FullAccess及正常内向junction/根alias正式链通过。生产写入/删除/移动/list/glob/grep操作期边界、完整链接链/根身份、UNC/非直接磁盘兼容、其他平台及全组/lint仍待，不能关闭H-78或H-77/H-79。当前native helper仅支持直接本地磁盘，该草稿限制不能作为最终FullAccess契约交付；尚无修复提交。

E-H78-20261026-05（首阶段补强，非整项验收）：将最深祖先解析改为迭代，避免缺失尾部逐级递归；随后builtins整组同命令再次退出0，319 passed/1 ignored（编译20.78s、测试25.91s）。追加正式ReadFileTool异步边界回归：持原路径gate，首poll确认工具已完成前置检查并派发worker，将父目录替换成外向junction后释放gate；真实工具拒绝、不返回外部合成正文、cache不登记外部读取资格；原文件停放路径的正常读取成功。`cargo test -p deepagent-builtins --lib read_tool_rechecks_path --offline -j 1 -- --nocapture`退出0，1 passed/0 ignored（编译15.76s、测试0.05s）。该夹具不声称worker当时已进入哪一条native指令；原生预检/open间隙另由E-04的确定性测试覆盖。`cargo clippy -p deepagent-builtins --lib --offline -j 1 --no-deps -- -D warnings`退出0（12.31s）；最终新增回归后的`cargo clippy -p deepagent-builtins --all-targets --offline -j 1 --no-deps -- -D warnings`退出0（4.66s）。当前仅阶段工作树，无修复提交；后续操作边界/兼容性及最终全组验收未完成，计数不变。

E-H78-20261026-06（目录阶段首测，非整项验收）：list_dir、glob、grep接入共享WorkspaceRoot读取边界；Windows目录通过同一native句柄枚举，不在授权后另用字符串read_dir；glob/grep使用显式DFS栈及当前祖先集合防联接循环，grep正文再次使用open_read。核对微软[GetFileInformationByHandleEx](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-getfileinformationbyhandleex)及[FILE_FULL_DIR_INFO](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_full_dir_info)的续读、变长字段及8字节对齐要求。`cargo fmt --all`退出0；`cargo test -p deepagent-builtins --lib file_tools::tests --offline -j 1 -- --nocapture`退出0，37 passed/0 failed/0 ignored（编译17.94s、测试0.14s）。本次仅既有文件工具回归，尚未证明跨页/Unicode枚举、预检后目录替换、循环和真实工具外向联接场景；随后补验证，不关闭H-78，也不顺手关闭H-77/H-79。此阶段未改写入/删除/移动边界，无提交。

E-H78-20261026-07（native目录专项）：`cargo fmt --all`退出0；`cargo test -p deepagent-builtins --lib native_directory --offline -j 1 -- --nocapture`退出0，5 passed/0 failed/0 ignored（编译13.95s、测试0.41s）。实测1200个含中文/非BMP字符的文件名加1子目录，名字字节总量已超过64KiB，完整枚举且无重复；打开原目录后rename并在原路径建立替代目录，旧句柄仍只返回original.txt，新开控制只返回replacement.txt；空目录成功、普通文件/缺失目标及不正确句柄失败，不包装成空成功；变长解析器对截断头、空/奇数字节/超长名字、NUL、错位/重叠/越界next拒绝并跳过点项。另以真实junction在物理预检后替换父目录：普通字符串read_dir确实返回outside.txt，native目录打开拒绝，停放原目录控制成功。仅本机临时合成数据。真实工具联接/循环与正式下一wire尚待补证，写操作等遗留边界不变；仍修复中。

E-H78-20261026-08（真实工具目录专项）：`cargo test -p deepagent-builtins --lib directory_tools_confine --offline -j 1 -- --nocapture`退出0，1 passed/0 ignored（编译13.55s、测试0.12s）；同夹具逐一验证Workspace/ReadOnly/ReadAnywhere/Full：list_dir外向联接只在允许外部读取的模式成功；glob/grep默认不返回外部文件名/正文，允许模式正常返回；内向联接及真实目录均保留结果，不用全局去重隐藏合法别名；真实回根联接不产生无限遍历（5秒超时保护且精确核对结果集）。`cargo test -p deepagent-builtins --lib discovery_preserves --offline -j 1 -- --nocapture`退出0，1 passed/0 ignored（0.02s）：普通目录DFS排序/截断、跳过既有隐藏/构建目录保持；根是普通文件时明确失败而非空成功。未据此关闭敏感名称等独立问题；正式下一wire与最终整组仍待。

E-H78-20261026-09（正式目录链首测失败，保留反证）：扩展既有离线JunctionModel以调用list_dir/glob/grep并观察配对function_call_output。`cargo test -p deepagent-app-core --test kernel_v2_e2e path_boundary --offline -j 1 -- --nocapture --test-threads=1`退出1，3 passed/1 failed/0 ignored（编译1m53s、测试2.03s）。既有read三组保持通过；新目录组list_dir/workspace为`content_returned=false, tool_completed_ok=None, tool_blocked=true`，但glob/workspace观察到外部标记（`content_returned=true, tool_completed_ok=Some(true), tool_blocked=false`），在此断言失败，后续模式/工具尚未跑到。直接Tool测试绿不能代替正式链；尚待定位标记来源/实际注册与权限映射，不将此处未定位失败改写为已修复或独立新增问题。

E-H78-20261026-10（定位正式链配置错配）：追加只打印合成夹具结果的诊断后，`cargo test -p deepagent-app-core --test kernel_v2_e2e path_boundary::directory_tool_results --offline -j 1 -- --nocapture --test-threads=1`退出1，0 passed/1 failed/0 ignored（编译1m02s、测试0.56s）。实际glob配对result.matches为`["inward/inside.txt","outward/outside.txt","real/inside.txt"]`，确认标记来自真实结果而非输入/包装误判。追踪`RunAssembler::base_registry_request`：主run固定传FsAccess::Full，subagent已传access；`assemble_run_hooks`另传有效access，PathGuard只检查path/file_path/filename，glob/grep隐式遍历不触发该参数检查。与H-78共享物理边界的接入直接相关，修复范围需包含run_assembler主注册，使其复用RunEnvironment已有access而非新增权限判定；FullAccess仍按既有映射为Full。H-77/H-79等独立项不归并、不关闭。另发现当前目录草稿把grep整批内容扫描放入不可中止blocking任务，需保留原逐文件async让步边界，避免因本项引入更大的取消后续读窗口；不是宣称解决全部取消问题。

E-H78-20261026-11（接入修正后的builtins整组）：主注册改传已有access；grep保留逐文件async等待并在每次实际内容打开时调用root.open_read，未把整批后续内容读取留在一个blocking worker内，不新增取消中心。`cargo fmt --all`退出0；`cargo test -p deepagent-builtins --offline -j 1 -- --quiet --test-threads=4`退出0，327 passed/0 failed/1 ignored（编译22.16s、测试25.90s），doc-tests0。目录阶段新增回归均默认执行，ignored不计通过。正式夹具另收紧观察器：只看read.content/list.entries/glob.matches/grep.hits.text，不扫描包含query的整个JSON，补query-only/error-only反例，防止grep回显查询产生假阳性；E-09/E-10的glob失败仍有真实matches证据，不因夹具改进而抹去。修改后的正式原链尚在编译，不提前登记通过。

E-H78-20261026-12（正式目录链阶段通过，lint仍有本轮失败）：修正主注册access及结构化观察器后，`cargo test -p deepagent-app-core --test kernel_v2_e2e path_boundary --offline -j 1 -- --nocapture --test-threads=1`退出0，5 passed/0 failed/0 ignored（编译1m49s、测试3.74s；含1个观察器反例测试，不是5个独立故障）。下一wire实测Workspace的list_dir硬拒绝、glob/grep成功但无外部匹配名/正文；FullAccess三工具均成功且有外部结果；内向联接/根alias及原read边界正常控制保持。P-H78-directory-repair（callflow）：RunEnvironment.access → 主registry与hook一致配置 → WorkspaceRoot授权 → Windows no-reparse打开/同句柄枚举 → glob候选/grep逐文件再打开 → 配对wire结构化结果；关联E-07～12。`cargo clippy -p deepagent-builtins --all-targets --offline -j 1 --no-deps -- -D warnings`退出1，指出本轮native测试缓冲区向上取整手写算式`(bytes+7)/8`触发manual_div_ceil；尚待改用仓库已有div_ceil用法后复验，不隐藏本轮引入告警。构建锁等待不计有效工时。H-78写入/删除/移动、完整链接链/根身份及UNC等兼容范围仍未完成，尚无修复提交，计数不变。

E-H78-20261026-13（修正本轮lint）：测试缓冲区改为与既有bash_tool相同的usize.div_ceil用法；本地Cargo.lock固定tokio 1.52.3的`task/blocking.rs`亦核对了blocking任务开始后不可abort的行为，未将网页版latest版本冒充本地版本。`cargo clippy -p deepagent-builtins --all-targets --offline -j 1 --no-deps -- -D warnings`退出0（6.87s）；`cargo test -p deepagent-builtins --lib native_directory --offline -j 1 -- --nocapture`退出0，5 passed/0 failed/0 ignored（编译13.98s、测试0.36s）。仅修正测试算式，无生产行为变化；跨层file_version/move_failure/hook_permissions回归仍在编译等待，不提前报结果。尚未运行本项最终workspace/全features/其他OS/真实UI/真实供应商，未作完整验收或提交。

E-H78-20261026-14（主注册变更的相关消费者回归）：以下命令均为`cargo test -p deepagent-app-core --test kernel_v2_e2e <filter> --offline -j 1 -- --nocapture --test-threads=1`，未启用ignored：`file_version`退出0，3 passed/0 failed/0 ignored（编译1m50s、测试2.79s），write/edit/multi_edit拒绝陈旧版本、重读后正常成功；`move_failure`退出0，2 passed/0 failed/0 ignored（测试1.26s），祖先目标拒绝保留源及旁文件，正常覆盖成功；`hook_permissions`退出0，4 passed/0 failed/2 ignored（测试1.81s），显式deny、workspace直接外部拒绝、普通文件控制保持，两个独立agent-hook已知问题仍ignored，不计通过、不关闭。本阶段不重做H-165或加密内容报错；未改生产用户数据、协议/前端DTO、schema、供应商或沙箱配置，未新增依赖版本。当前只工作树、未暂存提交；整项待完成写入/删除/移动实际I/O、完整链接链与根身份、UNC/SUBST/其他平台及最终workspace/性能验收。327项builtins整组及5项正式path_boundary均为阶段证据，不是全系统验收；总计仍11已验收/17历史跳过/7归并/2撤销/269有效未关闭。200–400有效小时仍是未经独立计量校准的原粗估，构建锁/设备等待另计，不从本轮墙钟推算吞吐。

E-H78-20261026-15（本阶段收尾核对，非整项验收）：最终源码再次执行E-12相同path_boundary命令，退出0，5 passed/0 failed/0 ignored（编译检查0.50s、测试3.47s）；`cargo fmt --all -- --check`退出0，`git diff --check`退出0（仅Git CRLF提示）；PowerShell重核台账306行/306唯一编号/无缺号/无错误列数，状态11已验收、17历史跳过、7归并、2撤销、1修复中、268待修，269有效未关闭。`git diff --cached --name-only`为空；原有未跟踪项未读改或暂存。当前只完成H-78读取/目录阶段，未创建commit，不将局部通过计作整项修复；下一步复用H-76提交边界与H-286 staging/rollback继续检查写入、删除和移动的实际I/O及条目语义，不能另建提交链。

E-H78-20261026-16（写入阶段原失败）：本轮先补真实公开Tool::invoke回归，持H-76原路径gate，首poll完成前检并派发worker后将父目录改名、原位置建立指向外部TempDir的junction，再释放gate；不假定worker具体native指令。外部同名文件故意与原文件字节相同，隔离H-76陈旧内容规则。`cargo test -p deepagent-builtins --lib mutation_tool_rechecks --offline -j 1 -- --nocapture --test-threads=1`退出1，0 passed/2 failed/0 ignored（编译28.25s、测试0.25s）：write/edit/multi_edit全部`ok=true, outside_unchanged=false, original_unchanged=true`；新文件组`ok=true, outside_created=true`。F-H78-mutation（P1，validated，confidence=high，evidence_ids=[E-H78-20261026-16]）：读取/目录阶段绿不覆盖mutation实际I/O，已有缓存授权可随父路径替换作用于同字节外部文件，新文件也能写到外部。P-H78-mutation：词法/物理前检 → 等待统一path gate → 普通字符串read_current/tempfile/rename跟随新junction → 外部合成文件被修改；未操作真实用户文件。后续必须在既有观察版本/提交边界中使用稳定目录/文件句柄，不能只再加一次canonicalize；H-76/H-286不重新计数。

E-H78-20261026-17（写入句柄阶段，非整项验收）：沿用H-76单一publish_checked版本/提交规则；gate后共享write_target授权，Windows从直接volume根逐组件保留不share-delete的TRAVERSE目录句柄，缺失父目录使用相对native创建，guard保持父目录非空并检查各句柄非reparse，最终再校验原路径授权；read_current/新暂存/发布均相对同一父句柄，失败和unwind仅按拥有的暂存句柄删除。参考本地Codex no_reparse_dir/file_write设计，未复制其业务代码；复用windows-sys 0.59和tempfile3.27既有绑定/API，无新增依赖。首编译退出1（9.58s），FILE_DISPOSITION_INFO.DeleteFile按bool填写与实际BOOLEAN/u8绑定不符；查本机源码后改1，保留此失败记录。原E-16命令复验退出0，2 passed/0 failed/0 ignored（编译19.09s、测试0.16s）：三种既有文件mutation均ok=false、内外不变，新文件ok=false且outside_created=false。这两项仅证明gate后父替换拒绝，不能替代native操作期竞态/兼容性验收。

E-H78-20261026-18（既有mutation边界回归）：`cargo test -p deepagent-builtins --lib file_mutation::tests --offline -j 1 -- --nocapture --test-threads=4`退出0，20 passed/0 failed/0 ignored（编译检查0.69s、测试5.05s），包含跨真实进程path gate、取消等待、版本/窗口、staging后变更、final no-replace竞争、发布失败及unwind清理。unwind用例预期panic被catch_unwind捕获，测试成功；不隐去该输出。测试注入仍走同一publish_checked，仅把底层提交对象替换为稳定WriteTarget/StagedWrite。后续待原地reparse/祖先及最终条目替换、正常内部联接/访问模式、delete/move、完整链接链/根身份、UNC/SUBST/其他平台和最终整组验收；H-78仍修复中，无commit，计数不变。

E-H78-20261026-19（整组发现本轮兼容回归，未验收）：`cargo test -p deepagent-builtins --offline -j 1 -- --quiet --test-threads=4`退出1，327 passed/2 failed/1 ignored（编译6.71s、测试25.89s）。write_then_read_roundtrip的src/a.txt新父目录写入失败；glob_and_grep_find_files的src下匹配为0（期望2），该用例先用write_file建文件且未检查写入ok，需沿新父目录创建追因，不能把后一失败先归为glob缺陷。已为原roundtrip断言补输出上下文，继续复验。此前E-17 gate替换拒绝/E-18既有版本规则成功未覆盖新建父目录，不能拿局部绿宣布mutation完成。

E-H78-20261026-20（新父目录回归根因与定向复验）：roundtrip补充错误上下文后，原命令`cargo test -p deepagent-builtins --lib write_then_read_roundtrip --offline -j 1 -- --nocapture`退出1，0 passed/1 failed（编译17.44s）：失败来自lock_path_operation要求parent已存在，并非native mkdir。原实现在gate前字符串mkdir才满足此隐藏前置条件；安全创建移到gate后暴露该耦合。将此前WorkspaceRoot最深现有祖先/缺失尾部解析收敛到persistence::fs_commit::canonicalize_with_missing_suffix，原权限预检与path gate复用同一实现；gate可在无副作用情况下定位未创建路径的稳定锁身份，不改既有父目录key，不新建第二套锁。添加缺失父创建前后identity相同及gate不创建目录的回归。相同roundtrip命令复验退出0，1 passed/0 failed/0 ignored（编译28.91s、测试0.02s）；整组、persistence及后续操作期测试尚待，不关闭H-78。

E-H78-20261026-21（操作期句柄与原地reparse对照）：`cargo test -p deepagent-builtins --lib write_target --offline -j 1 -- --nocapture --test-threads=1`退出0，3 passed/0 failed/0 ignored（编译30.25s、测试0.14s）。实测parent/ancestor/workspace在目标持有期rename均错误32，子文件仍可native发布；未发布stage按句柄清理、target释放后仅剩正式文件且ancestor可改名；目标末项在建立句柄后被换成外向/悬空junction时read_current报错，不能按新文件处理。第三项持预先取得的真实directory write handle，用FSCTL_GET_REPARSE_POINT读取合成junction内核buffer并原样FSCTL_SET：guard持有期错误145（目录非空），释放后同一handle/buffer成功；排除无权限/错误构造导致的假拒绝。仅临时目录，无真实数据；参考微软[reparse restrictions](https://learn.microsoft.com/en-us/windows/win32/fileio/reparse-points)与[GET](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-fsctl_get_reparse_point)，新增既有windows-sys Ioctl feature只供真实控制测试。该证据覆盖guard安装后的原地转换，不宣称证明所有安装前竞态/完整链接链；剩余范围不变。

E-H78-20261026-22（正常写入与共享解析回归）：`cargo test -p deepagent-builtins --lib mutation_new_nested --offline -j 1 -- --nocapture --test-threads=1`退出0，1 passed/0 failed/0 ignored（0.26s），真实内部junction下多级Unicode/非BMP新父目录在Workspace/ReadAnywhere/Full正常写入，ReadOnly拒绝；外向junction仅Full写入，其余拒绝且未造外部父目录；内外新父目录均无遗留guard/stage。`cargo test -p deepagent-persistence --offline -j 1 -- --quiet --test-threads=4`退出0，68单元+9 session_control集成passed/0 failed/0 ignored（编译14.99s，测试1.04s+5.29s，doc-tests0）；含新父创建前后共享路径身份稳定且gate无目录创建副作用。后续仅移动新增测试module到文件末尾避免测试后生产项布局；同测试语义不变，仍待最终lint/整组及跨层复验。

E-H78-20261026-23（写入阶段整组与lint首轮）：修复新父目录锁依赖后，`cargo test -p deepagent-builtins --offline -j 1 -- --quiet --test-threads=4`退出0，333 passed/0 failed/1 ignored（编译7.03s、测试25.85s，doc-tests0）；E-19两个正常场景恢复且原失败E-16现为默认回归。`cargo clippy -p deepagent-builtins -p deepagent-persistence --all-targets --offline -j 1 --no-deps -- -D warnings`首轮退出1（9.14s），本轮open_name的slice字节手算、volume_root有意rooted join触发两条lint；分别改用size_of_val和OsString追加根分隔符，不加allow或改成会丢drive的clippy建议。fmt已执行，最终lint与改后整组仍待；这是本轮引入告警，原记录不抹去。仍无整项验收/commit。

E-H78-20261026-24（lint修正及正式读取/目录链复验）：修正E-23两项后，相同双crate严格all-targets no-deps clippy退出0（9.06s）。`cargo test -p deepagent-app-core --test kernel_v2_e2e path_boundary --offline -j 1 -- --nocapture --test-threads=1`退出0，5 passed/0 failed/0 ignored（编译2m45s、测试4.20s）：原外向读Workspace硬拒绝，目录三工具Workspace无外部结果、Full正常外部结果，内向及根alias正常。该正式链在本轮生产实现上运行；之后仅补guard安装前转换的native测试及复用测试夹具helper，不将其尚未执行结果计入通过。H-78仍修复中，delete/move等边界不因上述绿而关闭。

E-H78-20261026-25（guard安装前转换窗口的定向实测）：复用已有合成junction夹具及内核GET/SET数据helper，直接构造与生产相同access/share的native父pin，先将空parent原地转换，再调用生产guard/validate方法；没有在生产增加故障回调或第二条写入链。`cargo test -p deepagent-builtins --lib write_target_guard --offline -j 1 -- --nocapture --test-threads=1`退出0，2 passed/0 failed/0 ignored（编译39.13s、测试0.09s）：安装前转换组guard创建失败（Win32错误1921，FilesystemLoop），validate拒绝且外部目录无任何guard或内容；原安装后转换/释放后成功对照仍通过。这是本机NTFS对handle-relative RootDirectory/no-reparse的实测，覆盖该定向窗口，不外推全部链路/其他文件系统。测试通过后继续正常版本正式链、最终builtins/lint；无整项验收或commit。

E-H78-20261026-26（正式mutation版本链复验及整套验证启动）：`cargo test -p deepagent-app-core --test kernel_v2_e2e file_version --offline -j 1 -- --nocapture --test-threads=1`退出0，3 passed/0 failed/0 ignored（编译2m28s、测试3.16s）：write/edit/multi_edit继续拒绝陈旧版本，重读后成功，正常未改变文件成功，原H-76契约保持。因共享fs_commit解析与builtins/AppCore跨crate接入，已启动项目要求的`cargo test --workspace --offline -j 1 -- --quiet --test-threads=4`，尚在编译；构建锁/编译等待不当作通过或有效实施工时。最终结果随后追加，本记录不预写成功。H-78仍待delete/move条目边界、完整链接链/稳定根身份、末项常规链接的替换语义、UNC/SUBST及其他平台/性能兼容核实；不关闭H-77/H-79/H-287等独立项。

E-H78-20261026-27（workspace实际失败与兼容自查，非整项验收）：E-26的workspace命令最终退出1（编译8m47s）。已运行部分中AppCore单元1132 passed/78 ignored（22.41s）、kernel_v2_e2e111 passed/89 ignored（30.20s）、builtins334 passed/1 ignored（25.94s）、persistence68+9 passed（1.35s+5.36s）。停止于未改动的deepagent-prompts::canvas_prompt::tests::every_node_kind_owns_exactly_one_profile：27 passed/1 failed，creative.video-edit.v1的defaultUserPrompt为空；对应原有独立H-306，不为本轮绿顺手修复。后续workspace targets/doc-tests未执行，不宣称workspace通过。最终新增测试后的双crate严格clippy退出0（10.17s）。另在最终源码核对发现本轮Stage Drop用了Rust1.82才稳定的&raw语法而manifest声明1.80；依据[1.82说明](https://blog.rust-lang.org/2024/10/17/Rust-1.82.0/)及[addr_of稳定性](https://doc.rust-lang.org/stable/std/ptr/macro.addr_of.html)改为已有同模块使用、1.51稳定且等价的ptr::addr_of!，无更改I/O行为。该一行在workspace之后改，最终builtins/lint待复验；未实际用1.80 toolchain编译，不声称已通过MSRV全项目检查。

E-H78-20261026-28（写入阶段最终源码复验，非H-78整项验收）：E-27等价指针语法修正后，`cargo fmt --all -- --check`退出0；`cargo test -p deepagent-builtins --offline -j 1 -- --quiet --test-threads=4`退出0，334 passed/0 failed/1 ignored（编译43.75s、测试25.92s，doc-tests0），新旧正常写入、原mutation父替换拒绝和安装前/后reparse控制均在最终实现运行。双crate all-targets严格no-deps clippy复验完成输出（9.58s），退出码待进程确认后追加；此前E-27同命令已退出0，不混写本次状态。workspace仅E-27范围已跑，受H-306早停，后续target与最终一行语法改后的全workspace未重跑；没有前端/协议/供应商/配置迁移，未运行真实UI或1.80 toolchain。尚无H-78修复commit，整体goal active，继续条目操作边界；不把阶段代码交付算第12项。

E-H78-20261026-29（条目阶段原失败；前段绿不覆盖delete/move）：E-28最终双crate严格clippy进程已确认退出0（9.58s）。随后新增真实公开Tool::invoke探针，以Tokio1.52.3已核实的max_blocking_threads(1)占住唯一blocking worker，工具完成同步路径前检并排队后，将parent改名、原位置创建外向临时junction，再释放worker；不用生产故障回调。`cargo test -p deepagent-builtins --lib entry_tools_recheck_parent --offline -j 1 -- --nocapture --test-threads=1`退出1，0 passed/1 failed/0 ignored（编译20.19s、测试0.15s）。四组独立TempDir：delete文件/目录均ok=true且外部合成条目消失，原停放条目保留；move文件/目录均ok=true、外部条目被移入工作区destination，原停放条目保留。F-H78-entry（P1，validated，high，E-29）：delete的字符串metadata/remove及move worker在排队后的字符串I/O仍跟随替换parent；P：前检→真实blocking排队→换parent→释放→外部合成条目删除/移动。仅操作新建临时文件，无真实用户数据。当前源码包含此默认失败回归，不能用E-28的334 passed冒充当前整组绿；条目生产修复尚未实施，仍在H-78内复用H-286 staging/rollback与共享稳定父边界，不另计独立项。

E-H78-20261026-30（条目首阶段修复及原探针转绿，非整项验收）：delete将重新授权/metadata/remove/验证收敛到既有单个blocking操作；共享entry_target保留末项条目名，仅将parent物理解析并复用WriteTarget的Windows祖先pin/guard，实际I/O使用稳定物理parent路径，避免把末项link误解析为要移动/删除的target。move在原H-286 validate/staging/发布/rollback边界前重新授权并持源/目标parent pins，初次ancestry拒绝仍在新父创建前；旧目标staging持同类guard跨发布和回滚，失败不自动递归删业务数据，正常清理前释放guard。`cargo test -p deepagent-builtins --lib entry_tools_recheck_parent --offline -j 1 -- --nocapture --test-threads=1`退出0，1 passed/0 failed/0 ignored（编译42.73s、测试0.17s），四组均ok=false/内外原条目不变/destination_absent=true。随后源码自查将staging业务I/O改用guard返回的物理parent，避免临时名字若在pin前变成合法内向link后继续用可变alias；这一步在该probe后改，最终版本回归待后续。参考[std remove_dir_all](https://doc.rust-lang.org/stable/std/fs/fn.remove_dir_all.html)的Windows no-follow递归机制，不自造第二套删除递归；尚未宣称完整递归竞态或全部entry兼容通过。H-286故障注入回归及新正常场景正在运行，H-78不关闭。

E-H78-20261026-31（保留H-286提交/回滚语义）：staging改用guard返回的稳定物理parent后，`cargo test -p deepagent-builtins --lib file_move::tests --offline -j 1 -- --nocapture --test-threads=1`退出0，12 passed/0 failed/0 ignored（编译25.54s、测试0.15s）：文件/非空目录覆盖、ancestor/case拒绝、final no-replace、staging/发布失败、成功rollback、rollback失败保留recovery_path、发布成功但cleanup警告及unwind保留旧目标均通过。预期unwind panic被捕获，非测试失败。另核对当前rustc1.96.0对应[Windows remove_dir_all源码](https://raw.githubusercontent.com/rust-lang/rust/1.96.0/library/std/src/sys/fs/windows/remove_dir_all.rs)：递归以父句柄相对NtOpenFile+OPEN_REPARSE_POINT处理条目；不把stable在线文档的未来版本替代当前工具链证据。该源码查证不是额外动态竞态证明。最终正常entry权限/链接场景、整组及lint仍待，H-78仍不关闭。

E-H78-20261026-32（正常链接控制发现条目语义缺口，保留失败）：`cargo test -p deepagent-builtins --lib entry_tools --offline -j 1 -- --nocapture --test-threads=1`退出1，1 passed/1 failed/0 ignored（编译28.72s、测试0.37s）。排队后父替换四组仍拒绝且内外不变；正常控制在Workspace的最终junction移动已成功且target保留，但随后delete该链接时错误5，原因是symlink_metadata的is_dir=false让旧删除选择remove_file，Windows目录junction需按所开条目身份删除。并未操作或删除junction target；ReadOnly及此前Workspace普通内部文件移动/删除已运行，其余模式因该失败尚未走完，不冒称全部控制通过。后续把最终条目的分类和删除绑定同一no-follow句柄，复用现有windows-sys FileDispositionInfo，避免靠symlink字符串/第二次canonicalize决定删除target；正常目录递归继续使用已查证std实现，不复制递归。该兼容问题在本项entry语义范围内处理，不新增H编号。

E-H78-20261026-33（正常entry控制转绿）：删除最终条目改为通过同一父句柄相对open（OPEN_REPARSE_POINT）后由该句柄metadata分类；文件/最终reparse用同一FileDispositionInfo删除，普通目录保留std递归。Stage失败清理复用相同mark_delete，不新增第二套原生删除定义。相同entry_tools命令退出0，2 passed/0 failed/0 ignored（编译29.45s、测试0.76s）：原排队替换四组保持拒绝；正常内部junction/根alias/Unicode新父目录的移动删除、ReadOnly拒绝、ReadAnywhere仍禁止外向写、Full外部允许，以及最终junction移动/删除只作用链接并保留target均通过。随后仅将write/entry的访问模式预检收敛为同一write_identity，把move缺失尾部解析复用persistence既有共享函数，移除重复递归实现；这之后的全组/lint及正式AppCore移动链复验待结果，不把此前绿直接盖到最终版本。

E-H78-20261026-34（恢复阶段结果与夹具失败）：已恢复并确认原进程83109退出0，`cargo test -p deepagent-builtins --offline -j 1 -- --quiet --test-threads=4`为337 passed/0 failed/1 ignored（编译29.07s、测试26.09s，doc-tests0），覆盖staging初始备份及失败rollback期间父目录rename被拒绝；此快照早于后来两项测试。恢复13975确认`entry_target_deletes`为1 passed/0 failed/0 ignored（编译32.18s、测试0.08s），证明取得内部物理parent后替换词法alias仍仅删除原内部条目。随后`cargo test -p deepagent-builtins --lib recursive_delete_unlinks --offline -j 1 -- --nocapture`退出1，0 passed/1 failed（编译10.39s、测试0.13s）：夹具`mklink`对`nested/outward`报Invalid switch，尚未执行生产删除；需把测试路径逐组件join后重测，不能登记成产品失败或通过。两项新增测试后的整组、lint、AppCore及最终跨crate验收仍待。

E-H78-20261026-35（递归条目控制复验）：仅将夹具路径改为`tree.join("nested").join("outward")`，未修改生产删除代码；相同`recursive_delete_unlinks`命令退出0，1 passed/0 failed/0 ignored（编译34.48s、测试0.04s）。公开DeletePathTool递归删除普通tree时只移除嵌套外向junction条目，外部TempDir中的合成keep.txt正文保留。该测试补充E-31的源码依据，证明此固定嵌套场景，不能代替所有并发namespace变化或其他平台验收。H-78仍修复中。

E-H78-20261026-36（条目阶段整组结果）：`cargo test -p deepagent-builtins --offline -j 1 -- --quiet --test-threads=4`退出0，339 passed/0 failed/1 ignored（编译检查0.77s、测试26.01s，doc-tests0），包含E-34/35两项及现有mutation/move回滚控制。随后新增隐藏中间link目标探针，尚未执行；本条整组结果只对应新增前源码。正式AppCore `move_failure`已启动编译，终态待确认；稳定根身份/完整链及UNC等兼容仍未完成，不关闭H-78，不提交阶段草稿。

E-H78-20261026-37（正式移动链通过，完整链原失败）：`cargo test -p deepagent-app-core --test kernel_v2_e2e move_failure --offline -j 1 -- --nocapture --test-threads=1`退出0，2 passed/0 failed/0 ignored（编译2m23s、测试1.40s），正式下一wire确认拒绝无覆盖、正常覆盖及祖先移动拒绝，源/同级条目/检查点恢复保持。新增`physical_preflight_rejects_hidden_intermediate_link_targets`后定向命令退出1，0 passed/1 failed（编译22.99s、测试0.15s）：Workspace的chain_read/chain_write均true；ReadOnly的chain_read也true；所有读模式的隐藏secret-store中间路径均被允许。夹具是workspace/chain→outside/back→workspace/real及workspace/alias→workspace/secret-store→real，最终文件始终在workspace；证明中间路径规则遗漏，不声称读到外部秘密。根因是canonicalize每个原始词法前缀仍吞掉同一前缀内的多跳。静态查证Claude `utils/fsOperations.ts::getPathsForPermissionCheck`收集中间link目标并有40跳边界；该机制不等同于原子I/O，未执行参考项目测试。随后将共享预检补为逐跳检查，保留既有native操作边界；H-78不关闭。新测试的fmt --check曾提示布局差异，后续需fmt及复验。

E-H78-20261026-38（完整链定向复验）：共享check_physical现在逐组件检查即刻link目标，重新排队相对/绝对目标，并对中间敏感路径、离开根路径及超过40跳显式拒绝；最终解析结果须与首次物理身份一致。仍复用原native open/pin，链检查不冒充原子快照。`cargo test -p deepagent-builtins --lib physical_preflight --offline -j 1 -- --nocapture`退出0，4 passed/0 failed/0 ignored（编译26.86s、测试0.21s）：E-37隐藏链在ReadOnly/Workspace拒绝，ReadAnywhere只允许读、Full允许普通外部链，隐藏敏感目标在所有模式拒绝；原正常内部alias、悬空、先出后回及预检后替换控制通过。官方read_link/symlink_metadata文档已核对；当前在线文档版本1.99.1，仅用于稳定API约定，不冒充本机1.96源码行为。该共享逻辑变更后的整组/lint和AppCore尚待，稳定根身份等仍未完成。

E-H78-20261026-39（新实现的盘根兼容回归）：`cargo test -p deepagent-builtins --lib unconfined_volume_root --offline -j 1 -- --nocapture`退出1，0 passed/1 failed（编译30.84s、测试0.01s）。只对TempDir所在盘根做身份解析，不枚举/读取真实文件；Full模式对普通盘根拼写报physical path changed。根因是逐跳walker无Normal组件时未取得canonical拼写，`C:\`与`\\?\C:\`比较不等。随后对walker最终结果统一走既有physical_with_missing_suffix，不放宽权限、不fallback路径I/O；定向与整组结果待复验。本条明确保留本轮引入的兼容失败。

E-H78-20261026-40（盘根回归转绿与最新验证启动）：统一walker最终canonical拼写后，`cargo test -p deepagent-builtins --offline -j 1 -- --quiet --test-threads=4`退出0，341 passed/0 failed/1 ignored（编译36.77s、测试26.20s，doc-tests0），包含E-37隐藏链及E-39普通盘根控制。`cargo clippy -p deepagent-builtins -p deepagent-persistence --all-targets --offline -j 1 --no-deps -- -D warnings`退出0（8.71s）。随后仅新增AppCore正式下一wire的隐藏链对照测试（Workspace拒绝外出返回、Full允许普通链、所有模式敏感链拒绝、正常内部两跳成功），并执行fmt；该新增测试尚未得到结果。已启动最新共享逻辑的workspace默认全组，待确认终态；不沿用E-27快照宣称最新跨层通过。静态复核还需覆盖link目标恰为root祖先、再经后缀返回的情形，尚未证明此形状，下一步定向复现。

E-H78-20261026-41（最新workspace真实失败及定位）：`cargo test --workspace --offline -j 1 -- --quiet --test-threads=4`退出1（编译8m07s）：AppCore1132 passed/78 ignored（19.52s），kernel_v2_e2e112 passed/89 ignored（36.88s，包含新隐藏链正式wire控制）；builtins341 passed/1 failed/1 ignored（25.97s），失败即新默认探针`physical_preflight_rejects_link_to_root_ancestor_even_when_suffix_returns`，Workspace实测permitted=true。本次先停在本轮缺口，尚未到H-306或后续crate/doc-tests，不能称仅原基线失败。根因是walker为遍历绝对目标临时允许root的祖先，但未在“即刻link目标结束、原路径后缀继续”处校验边界，导致up→root.parent再接root.basename返回。需用单一walker的目标结束标记校验完整目标，不叠加特例或禁止正常Full链。独立台账重核306行/306唯一编号/无缺号/8列，状态仍11已验收/17历史跳过/7归并/2撤销/1修复中/268待修；E-34～40两文逐条一致。

E-H78-20261026-42（目标结束边界定向通过）：walker以LinkTargetEnd标记在每个即时目标结束、拼接原后缀之前执行完整containment检查；祖先遍历仅用于到达绝对目标，不能把祖先本身当授权目标。`cargo test -p deepagent-builtins --lib physical_preflight --offline -j 1 -- --nocapture`退出0，5 passed/0 failed/0 ignored（编译27.22s、测试0.23s）：原E-41 Workspace祖先返回被拒绝，Full正常允许，E-37隐藏链/敏感链及原正常控制通过。该最终walker之后的整组、严格lint与AppCore尚待；接续调查已列出的稳定根身份，仍只处理H-78。

E-H78-20261026-43（稳定根身份原失败，非旧探针重复）：新增`workspace_root_authorization_does_not_rebase_after_root_replacement`，使用两个独立TempDir保存合成原/替换文件，先创建WorkspaceRoot并完成resolve_read，再通过clone执行open_read。`cargo test -p deepagent-builtins --lib workspace_root_authorization --offline -j 1 -- --nocapture`退出1，0 passed/1 failed（编译21.27s、测试0.10s）：普通根改名后用外向junction替换、原根本为alias后改指另一TempDir，两组permitted=true；未读取正文，只取得可读句柄，内外文件正文事后核对不变。根因是每次check_physical重新canonicalize root，把新的物理目标当成授权基准，而不是run/tool root原身份。已核对本机windows-sys0.59的FileIdInfo/FILE_ID_INFO及微软目录handle文档；后续需统一持有初始根/最深现有祖先身份并在实际I/O期间验证，不能仅缓存字符串或仅在首次调用建立信任，保留原根alias和缺失根正常创建契约。当前新默认回归为真实失败，H-78仍修复中，未实现/未宣称该根身份修复，无commit。

E-H78-20261026-44（本轮最终快照，仍未整项验收）：最新`cargo test -p deepagent-builtins --offline -j 1 -- --quiet --test-threads=4`退出1，342 passed/1 failed/1 ignored（编译9.06s、测试26.22s），唯一失败为E-43稳定根身份原失败；新增walker的隐藏目标/祖先返回、盘根以及全部其他默认回归通过。最新双crate严格all-targets no-deps clippy退出0（11.59s）；fmt --check及git diff --check退出0（Git仅CRLF提示）。最新workspace仅E-41快照，当前最终walker仍需正式AppCore及最终跨crate重验；不隐去其失败，不以lint成功代替行为验收。没有暂存/commit，不改原五类未跟踪项。H-78后续按稳定根身份→常规最终文件link的mutation语义→UNC/SUBST与其余平台/性能边界继续；整体goal active，本阶段新增整项验收0，计数保持11已验收/17历史跳过/7归并/2撤销/269有效未关闭。旧200–400有效小时仍未经样本校准，编译与设备等待另计，不机械扣减。

E-H78-20261026-45（根身份首阶段定向转绿）：WorkspaceRoot构造时捕获原物理根/最深存在祖先的保留File，clone共享同一anchor；Windows使用已核对的volume serial+128-bit FileId，缺失根首次创建后以OnceLock绑定根自身身份。操作期只读pin复用既有WriteDirectory逐组件no-reparse/TRAVERSE逻辑，不创建guard文件；read/目录在native实际打开/枚举期间保留pin，mutation/entry在创建目标父和复核期间保留pin。原`workspace_root_authorization`命令退出0，1 passed/0 failed/0 ignored（编译38.45s、测试0.12s），普通根/根alias替换两组permitted=false。此结果对应首次接入快照；随后复核发现check_physical仍重新取得root路径作为containment基准，已改为受限模式使用anchor.physical，并让write/entry持pin覆盖其创建阶段；这之后完整回归尚待。Unix仅加入dev/ino身份比对，未建立操作期抗rename保证；UNC/非直接盘与其他已有未验范围继续保留。不关闭H-78。

E-H78-20261026-46（根身份与正常创建控制）：最终受限containment改用anchor.physical，write/entry持根pin覆盖parent创建及复核后，`cargo test -p deepagent-builtins --lib workspace_root --offline -j 1 -- --nocapture`退出0，5 passed/0 failed/0 ignored（编译22.56s、测试0.11s，包含1个既有禁止删root测试）。同名普通目录替换在首次使用前也被拒绝，clone不重置信任；ReadOnly读取/目录和Workspace/ReadAnywhere写入均拒绝旧root的新身份，新构造root可明确授权新目录。初始缺失的Unicode多级root通过公开WriteFileTool正常创建，创建后替换同名目录再次写入被拒绝且替换目录零条目；只读root pin阻止rename（错误32），根内不生成guard文件，释放pin后rename正常。原普通根/alias替换两组保持permitted=false。当前仅定向阶段通过，完整builtins/lint、正式AppCore及最终workspace仍待，不关闭H-78。

E-H78-20261026-47（根身份整组及lint阶段）：`cargo test -p deepagent-builtins --offline -j 1 -- --quiet --test-threads=4`退出0，346 passed/0 failed/1 ignored（编译7.70s、测试25.97s，doc-tests0），E-43原失败及根缺失/clone/首次前同名替换/只读pin对照均纳入默认全组。`cargo clippy -p deepagent-builtins -p deepagent-persistence --all-targets --offline -j 1 --no-deps -- -D warnings`退出0（9.32s）。当前根身份控制已定向及builtins验证；正式AppCore/最终workspace尚待，普通最终file symlink mutation语义、UNC/SUBST及其他平台/性能仍待核实，不能将346项绿计成H-78整项验收。接续核对HEAD既有mutation的entry发布语义，无新H项。

E-H78-20261026-48（末项文件link语义真实回归）：核对HEAD `file_mutation::publish`以原parent的临时文件通过MoveFileExW(REPLACE_EXISTING|WRITE_THROUGH)替换原条目。新增真实本机symlink_file夹具及公开read_file→write_file（同一cache）后，`cargo test -p deepagent-builtins --lib mutation_final_file_symlink --offline -j 1 -- --nocapture`退出1，0 passed/1 failed（编译25.76s、测试0.04s），本机创建symlink成功；实际target="replacement fixture"、alias_is_link=true。说明本轮write_target整路径canonicalize把原entry发布改成写target，不能把该兼容回归当作预期安全修复。后续发布复用entry_target只解析/pin parent，观察正文仍走同一WorkspaceRoot写权限与逐跳授权/no-reparse读取；H-76原版本比较/提交边界不重建。保留原失败，并补与HEAD相同native调用的正常对照，不关闭H-78。

E-H78-20261026-49（保留本轮编译失败）：把mutation观察与发布分离后，同一`mutation_final_file_symlink`命令退出1，未运行测试（编译6.53s）：原失败/unwind清理测试的catch_unwind要求RefUnwindSafe，但WriteTarget新增携带WorkspaceRoot后，anchor内通用io::Error的dyn Error使其不满足该约束。不是运行时link回归验收，也没有删除unwind测试或添加生产unsafe实现；随后将只读root捕获失败保存为ErrorKind+原错误Display文本，保留已有对外错误上下文，并移除permission snapshot内任意dyn Error对象。复验及最终整组结果待确认。

E-H78-20261026-50（末项file link发布对照转绿）：捕获错误改为ErrorKind+完整Display文本后，同一link命令首先1 passed（编译24.56s、测试0.05s），target="original fixture"、alias_is_link=false。随后增加与HEAD完全相同MoveFileExW flags的本地native正常对照，相同命令2 passed/0 failed/0 ignored（编译19.89s、测试0.04s），证明既有entry替换语义并非猜测。生产write_target复用entry_target的稳定parent及原条目名字，WriteTarget的mutation正文观察统一使用原logical路径、WorkspaceRoot写权限/完整链/no-reparse读取，发布仍是H-76 publish_checked；未绕过ReadAnywhere写限制。此快照只覆盖write_file及native对照，随后把正常file link测试扩为write/edit/multi_edit三工具，扩展结果和全组/lint/AppCore尚待。

E-H78-20261026-51（全组暴露dangling错误分类回归）：扩展三种mutation后，`cargo test -p deepagent-builtins --offline -j 1 -- --quiet --test-threads=4`退出1，347 passed/1 failed/1 ignored（编译26.71s、测试25.92s）。三工具file link正常替换和native对照通过；唯一失败是既有`write_target_rejects_final_entry_changed_to_outward_or_dangling_junction`：观察器把授权阶段产生的NotFound（悬空link）误合并为“新文件不存在”。后续将不存在分类仅放在统一授权成功之后的实际native open，对授权错误直接传播，保留该默认回归；不靠错误字符串特判或放宽dangling限制。另只读核对可用环境：已装Windows MSVC及两种Android Rust target，WSL存在Ubuntu/docker-desktop，尚未执行Unix构建/测试，不把安装状态当验收。H-78仍修复中。

E-H78-20261026-52（dangling分类复验及最新整组）：授权错误不再进入native缺失分类后，最新同一builtins全组命令退出0，348 passed/0 failed/1 ignored（编译23.93s、测试25.93s，doc-tests0）。原dangling/outward末项变化拒绝重新通过；write/edit/multi_edit三工具file symlink均替换alias条目并保留原target正文，HEAD native对照、根身份及既有失败/unwind/回滚控制同组通过。WSL Ubuntu只读PATH探测只返回Linux，cargo/rustc未找到，尚未运行Unix构建，不宣称全平台通过。随后只新增AppCore第一模型请求时替换已组装registry根目录的正式wire测试（普通同名新目录/外向junction两组），已启动path_boundary全组编译，结果待确认；该测试注入只在离线模型fixture，不加生产故障回调。H-78仍待跨层及UNC等兼容范围，不提交阶段草稿。

E-H78-20261026-53（正式根替换及路径链通过）：`cargo test -p deepagent-app-core --test kernel_v2_e2e path_boundary --offline -j 1 -- --nocapture --test-threads=1`退出0，7 passed/0 failed/0 ignored（编译2m28s、测试6.24s）。离线模型首次请求时（真实registry已组装）把根改名并分别换成普通同名新目录/外向junction，两组下一wire均content_returned=false、tool_completed_ok=None、tool_blocked=true，原根/外部合成正文保持；正常内部根alias、两跳内部link、Full普通外部链及目录工具控制保持。该7项含观察器反例，不冒称7个独立故障。随后静态全局搜索确认mutation已统一使用带写授权的正文观察，删除WriteTarget中不再有生产调用的旧平台读取fallback和Windows低层重复read_current；未新增第二条版本/提交链。该收敛后的builtins/lint及正式链复验待结果，不用本次快照冒充后续源码。

E-H78-20261026-54（最终收敛后lint与兼容依据）：删除旧读取fallback后的双crate严格all-targets no-deps clippy退出0（9.20s）；已启动`cargo test --workspace --offline -j 1 -- --quiet --test-threads=4`（当前进程92841仍在编译），结果未返回，不能当通过。静态再读Codex exec-server/src/no_follow/windows.rs的nt_path：它显式处理Disk/VerbatimDisk/UNC/VerbatimUNC并设置SecurityIdentification；本地disk_object_name当前仍明确拒绝UNC及非直接HarddiskVolume映射。参考源码不是可直接复制的本机运行证明，未执行参考工程/SMB服务，不能据此认定当前Full契约已验收。SUBST实际经canonicalize后的形状尚待动态确认，不能仅凭底层拒绝非直接映射就断言全部SUBST工具路径失败。WSL Ubuntu额外只读检查/root及/home下cargo入口未找到；无Linux构建/执行证据，其他平台能力仍未关闭。台账摘要更新到本轮真实阶段，计数不变，无暂存/commit。

E-H78-20261026-55（收敛后workspace真实终态）：E-54的进程92841已退出1，编译7m44s；AppCore lib 1132 passed/78 ignored，正式kernel_v2_e2e 113 passed/89 ignored（30.34s），builtins 348 passed/1 ignored（26.04s），persistence lib 68 passed及session_control 9 passed。随后仍在既有H-306 `canvas_prompt::tests::every_node_kind_owns_exactly_one_profile`失败：creative.video-edit.v1的defaultUserPrompt为空，prompts为27 passed/1 failed；因此workspace不通过，后续crate/doc-tests未执行，不能沿用此前独立通过补成全局绿灯。此运行验证的是删除旧mutation读取fallback后的生产源码；新加的SUBST/本机UNC兼容夹具不在该349项builtins二进制中，另启定向命令验证。已只读确认本机WSL UNC临时目录可访问，兼容测试仅新建TempDir；暂无UNC通过结论，H-78不关闭，无提交。

E-H78-20261026-56（namespace兼容实测，保留本轮UNC回归）：新增共享公开文件工具roundtrip及真实SUBST式DOS盘符映射（QueryDosDevice检查空闲盘符、DefineDosDevice精确目标清理），UNC夹具仅在显式DEEPAGENT_H78_UNC_ROOT下新建TempDir，默认ignored不冒称通过。`$env:DEEPAGENT_H78_UNC_ROOT='\\wsl.localhost\Ubuntu\tmp'; cargo test -p deepagent-builtins --lib namespace_public_file_tools --offline -j 1 -- --include-ignored --nocapture --test-threads=1`退出1，1 passed/1 failed/0 ignored（编译31.25s、测试0.19s）。SUBST在Workspace/Full两模式下read/write/list/glob/grep/move/递归delete及Unicode新父路径全部通过，canonicalize实际转回原C盘物理目录；UNC canonicalize返回VerbatimUNC，公开read在root capture因`native file open requires a local disk path`失败。不能把低层拒绝SUBST等同高层不可用，也不能把UNC拒绝作为最终契约。两份文档即时保留此回归；随后只在统一Windows native路径边界处理兼容，不另建工具链。

E-H78-20261026-57（UNC/SUBST修复后验收）：将Windows native open改为“DOS/UNC根一次解析 + 后续组件从已授权根句柄相对NtCreateFile/OBJ_DONT_REPARSE打开”，补SecurityIdentification；FileIdInfo不被远端provider支持时，回退同一句柄的公开GetFileInformationByHandle volume serial+file index，不使用不稳定Rust MetadataExt API。真实 DEEPAGENT_H78_UNC_ROOT='\\wsl.localhost\Ubuntu\tmp' 夹具执行 namespace_public_file_tools 退出0，2 passed/0 failed/0 ignored（SUBST与UNC各一项；UNC实际canonicalize为VerbatimUNC）。两项公开roundtrip均覆盖read/write/list/glob/grep/move/递归delete与Unicode新父路径。随后 builtins 全组退出0，349 passed/2 ignored，doc-tests0；严格 clippy 退出0（2m32s）；AppCore path_boundary 退出0，7 passed/0 failed（6.13s）。workspace总测此前仍在H-306失败，不能改写为全局通过。UNC依赖显式本机测试share，未访问真实用户数据/内网；其他OS、跨卷、断电与完整竞态仍待，H-78继续修复中。

E-H78-20261026-58（AppCore mutation消费者回归）：UNC/SUBST native边界变更后，`cargo test -p deepagent-app-core --test kernel_v2_e2e file_version --offline -j 1 -- --nocapture --test-threads=1`退出0，3 passed/0 failed（2.59s）：write/edit/multi_edit stale read拒绝、重读后允许、外部内容不被覆盖。`cargo test -p deepagent-app-core --test kernel_v2_e2e move_failure --offline -j 1 -- --nocapture --test-threads=1`退出0，2 passed/0 failed（1.30s）：祖先目标拒绝且源/sibling保留，普通替换及恢复语义保持。两项确认H-76版本提交边界与H-286 move staging/rollback消费者没有被本轮物理路径改动破坏；H-78仍不关闭。

E-H78-20261026-59（补记兼容阶段真实失败及依据收紧）：E-57之前的首次复验漏传DEEPAGENT_H78_UNC_ROOT，结果1 passed/1 failed（18.27s编译，失败在夹具前置条件）；补环境后仍1 passed/1 failed（0.19s），本机WSL provider的FileIdInfo返回错误50。尝试Rust MetadataExt::volume_serial_number/file_index曾编译失败E0658（windows_by_handle不稳定），未使用nightly或隐藏失败；改为既有windows-sys公开GetFileInformationByHandle后得到E-57结果。再核对微软[GetFileInformationByHandle](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getfileinformationbyhandle)与[BY_HANDLE_FILE_INFORMATION](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/ns-fileapi-by_handle_file_information)：网络provider可能只返回部分信息，ReFS应使用128-bit ID。当前因此只在明确查询不支持错误回退，旧接口零index拒绝，新旧身份独立enum，避免无条件降级及混合表示误等；不声称任意网络文件系统可稳定标识。新增UNC同名root替换前后File身份对照；该最新收紧版本已启动定向复验，结果尚待。native根句柄+子组件no-reparse是本地适配设计，参考Codex两种native边界及微软接口依据，未直接复制业务代码/运行参考工程。

E-H78-20261026-60（身份回退收紧后结果及阶段核对）：E-59最新版本的显式UNC/SUBST命令退出0，2 passed/0 failed/0 ignored（编译19.83s、测试2.24s）；UNC夹具包含首次使用前普通根改名、同名新目录替换拒绝及新建WorkspaceRoot明确授权正常对照。随后builtins全组退出0，349 passed/0 failed/2 ignored（编译7.83s、测试25.91s、doc-tests0），严格双crate clippy退出0（11.22s）。两项ignored中UNC已由上述显式命令真实执行，另一项仍不计通过。此时没有运行新的workspace全组，E-55已有H-306阻断；不声称后续crate或其他OS通过。AppCore file_version/move_failure证据仍是E-58的前一快照，最后修改只收紧内部身份表示/回退错误分类，未改公开工具/协议契约。无暂存或commit，H-78未整项验收；继续保留环境及竞态/性能验收待办。

E-H78-20261026-61（本机路径开销抽样，非性能承诺）：新增默认ignored的公开ReadFileTool计时夹具，8层相对路径、51,200字节，5次预热后各40次读取，Full和Workspace使用同一当前实现；每次断言真实工具成功，H-76仍实际读取完整字节。首次以短名配--exact误筛成0 tests（352 filtered out），不计验收；改为完整名执行`cargo test -p deepagent-builtins --lib fs_guard::namespace_tests::namespace_read_authorization_timing_probe --offline -j 1 -- --ignored --exact --nocapture --test-threads=1`退出0，1 passed（0.47s）。实测Full median/p95/max=4032/5332/5979µs，Workspace=5621/7019/7059µs；仅说明本机当前受限模式的增量成本约1.6ms中位数，不是HEAD前后性能对比，不外推冷盘/大仓库/UNC/并发分位数。最新新增仅ignored计时测试，不改变生产源码；默认全组最近实际执行仍是E-60的349 passed/2 ignored，不把新增ignored数冒充重新执行。台账核对306行/306唯一/无缺号/8列正确，11已验收、17历史跳过、7归并、2撤销、1修复中、268待修，269有效未关闭；无暂存/commit。

E-H78-20261026-62（补齐Linux验证环境，尚非通过）：本轮重新核对当前工作树/HEAD=8d8260e0及CI三平台矩阵。此前WSL shell循环未准确展示工具入口；逐命令确认Ubuntu24.04.1有curl/python3/ld，但无Rust与C/C++编译器。经Rust官方rustup安装文档及安装器help核对，在独立/tmp/deepagent-h78-runtime.CElDXo下安装固定Rust1.96.0（与Windows版本对齐），使用--no-modify-path，不改用户默认Rust/配置；官方rustup-init的SHA256校验通过。WSL内执行apt模拟后安装gcc/g++/make/pkg-config及所需依赖，真实37个新包、0升级/0删除，约227MB；编译器实测13.3.0。该环境变更用于运行项目当前源码的Linux兼容测试，不能代替测试通过；准备复用已有公共crate archive/index缓存、构建产物独立放在本轮/tmp，未读取凭据或原有未跟踪用户文件。macOS和断电环境仍不可用，H-78仍修复中。

E-H78-20261026-63（Linux真实结果与启动失败）：首次通过cargo proxy启动时，仓库rust-toolchain.toml指定stable，覆盖临时目录内已安装的1.96.0选择，触发stable1.99.1下载；并行help/test争用下载导致其中test启动因rustc归档rename失败退出1，未运行项目测试。随后显式使用/tmp/deepagent-h78-runtime.CElDXo/rustup/toolchains/1.96.0-x86_64-unknown-linux-gnu/bin/cargo及同目录RUSTC，执行`wsl.exe -d Ubuntu --cd /mnt/g/Code_Warehouse/DeepAgent-Studio -- env CARGO_HOME=/tmp/deepagent-h78-runtime.CElDXo/cargo CARGO_TARGET_DIR=/tmp/deepagent-h78-runtime.CElDXo/build RUSTC=/tmp/deepagent-h78-runtime.CElDXo/rustup/toolchains/1.96.0-x86_64-unknown-linux-gnu/bin/rustc /tmp/deepagent-h78-runtime.CElDXo/rustup/toolchains/1.96.0-x86_64-unknown-linux-gnu/bin/cargo test -p deepagent-builtins --lib --locked --offline -j 4 -- --quiet --test-threads=4`退出0：编译1m03s，302 passed/0 failed/0 ignored（5.04s）。这是当前生产源码在WSL Ubuntu上的实际测试，不代表macOS或Unix操作期抗rename已完成；Unix仍存在字符串I/O边界限制。临时stable下载未修改默认用户toolchain。H-78仍修复中，下一步真实双NTFS卷公开MovePathTool失败/回滚验收。

E-H78-20261026-64（真实跨卷通过，Linux文档测试启动失败如实保留）：Get-Volume核对C:/G:为不同UniqueId的独立NTFS卷。新增显式ignored夹具cross_volume_public_move_preserves_source_and_previous_target，仅在DEEPAGENT_H78_CROSS_VOLUME_ROOT提供授权本地目录时运行，先以真实fs::rename确认ERROR_NOT_SAME_DEVICE=17，再经公开MovePathTool逐组测试文件/目录×目标缺失/overwrite旧目标。执行`$env:DEEPAGENT_H78_CROSS_VOLUME_ROOT='G:\Code_Warehouse\DeepAgent-Studio\target'; cargo test -p deepagent-builtins --lib cross_volume_public_move --offline -j 1 -- --include-ignored --nocapture --test-threads=1`退出0：1 passed/0 failed/0 ignored（编译30.21s、测试0.06s），四组均真实move failed/os error17；源正文保留，两组覆盖均destination_restored=true且旧正文保留，无recovery_path及owned staging/guard残留。未新增跨卷复制语义。另Linux执行与E-63相同独立cargo/RUSTC环境的`cargo test -p deepagent-persistence --locked --offline -j 4 -- --quiet --test-threads=4`：68个lib及9个session_control通过，随后因PATH无rustdoc而doc-test进程未启动，命令整体退出1；这是环境入口遗漏，不能记作整crate通过，接下来显式RUSTDOC仅补跑文档测试。H-78仍修复中；macOS/断电及最终全组复核未完成。

E-H78-20261026-65（当前默认整组与Linux文档入口复验）：新增跨卷及计时ignored夹具后的Windows `cargo test -p deepagent-builtins --offline -j 1 -- --quiet --test-threads=4`退出0：349 passed/0 failed/4 ignored（编译检查1.69s、测试25.95s），doc-tests0。四项ignored不并入默认通过数，跨卷/UNC/计时已有各自显式运行证据；既有其他ignored仍未验收。Linux仅补齐RUSTDOC=/tmp/deepagent-h78-runtime.CElDXo/rustup/toolchains/1.96.0-x86_64-unknown-linux-gnu/bin/rustdoc，其余沿E-63的独立cargo/build环境，执行`cargo test -p deepagent-persistence -p deepagent-builtins --doc --locked --offline -j 4 -- --quiet`退出0（编译16.10s），两个crate均0 doc-tests，不冒称新增测试。E-64完整命令的退出1记录保留。尚在执行包含http可选feature的双crate严格lint，未得到终态；H-78状态和计数不变。

E-H78-20261026-66（最终正式读取链复验与mutation补强启动）：`cargo clippy -p deepagent-builtins -p deepagent-persistence --all-targets --all-features --offline -j 1 --no-deps -- -D warnings`退出0（11.88s），覆盖两个受影响crate及http可选feature。当前生产源码执行`cargo test -p deepagent-app-core --test kernel_v2_e2e path_boundary --offline -j 1 -- --nocapture --test-threads=1`退出0，7 passed/0 failed/0 ignored（编译2m33s、测试6.05s），原外向/隐藏目标/同名根替换拒绝、Full及内向控制保持。最终核对发现正式mutation消费者仍只有版本/回滚证据，尚无junction授权对应磁盘+下一wire的直接证据，因此只扩展现有path_boundary离线夹具：write/delete/move双向路径，内向/外向junction×Workspace/Full共16组；审批固定FullAccess以隔离实际filesystem profile，仍经真实RunAssembler注册/权限/工具/事件和paired output，不绕过生产链。新增测试已启动，未获得结果；不将既有7项通过冒充包含新测试。H-78仍修复中，无提交。

E-H78-20261026-67（正式mutation首测失败，保留夹具反证）：新增16组后相同path_boundary命令退出1，7 passed/1 failed/0 ignored（编译1m45s、测试8.17s）。外向Workspace的write/delete/move两方向均被拒绝；外向Full write成功，随后Full delete的下一wire成功字段及tool_completed.ok均为true，但夹具观察到tool_blocked事件而断言失败，后续组未执行。这不构成越界读写证据，也不能当16组已通过。核对RuntimeEvent::ToolBlocked和tool_pipeline::request_approval：该事件同时表示needs_approval=true的待审批与false的终结拒绝，原只读观察器将二者混作硬拒绝；接下来仅补打印合成夹具的审批/拒绝事件字段并按事件契约区分，不修改生产审批逻辑，不靠删除磁盘或wire断言取得绿灯。H-78仍修复中。

E-H78-20261026-68（正式mutation与磁盘/wire验收通过）：按ToolBlocked公开事件契约仅将needs_approval=false计为硬拒绝后，同一path_boundary命令退出0，8 passed/0 failed/0 ignored（编译1m35s、测试12.62s）。新增16组全部实际执行：外向Workspace write/delete在path hook硬拒绝，move源/目标两方向经过高风险审批后工具失败且源/外部目标保持；Full与内向Workspace/Full正常控制全部完成。每组同时断言磁盘正文/存在性、RunStore配对tool_completed及下一次真实序列化请求中对应function_call_output的成功字段；不以echo参数或工具声明代替磁盘。事件实测Full delete/move的先前tool_blocked均needs_approval=true（high-risk requires approval），后续成功，没有终结硬拒绝；E-67原失败保留。`cargo fmt --all -- --check`退出0（3.79s）。本轮生产源码未再变更；准备核对AppCore定向lint与版本消费者的最后结果，再整理Windows H-78的明确验收范围及未执行环境限制；不宣称workspace已通过或Unix竞态已修复。

E-H78-20261026-69（AppCore严格lint基线限制与版本复验）：当前源码 `cargo test -p deepagent-app-core --test kernel_v2_e2e file_version --offline -j 1 -- --quiet --test-threads=1`退出0，3 passed/0 failed/0 ignored（命令含构建锁等待29.37s、测试3.24s）；未将等待记入有效实施工时。`cargo clippy -p deepagent-app-core --test kernel_v2_e2e --offline -j 1 --no-deps -- -D warnings`退出1，在lib阶段遇到既有10项：context_runtime.rs八处clone_on_copy、plugin_preparation.rs:14 type_complexity、run_assembler.rs:1649 base_registry_request的too_many_arguments，和此前H-22基线相同；本轮只改后者调用参数access，没有改该函数签名，不顺手修无关项。lib阻断意味着尚未lint到integration target，不能冒称本轮测试代码无告警。随后只收敛本轮测试helper的参数和直接返回表达式，启动保留warning的同target clippy继续核对；该命令不是严格lint通过。修复状态/计数不变。

E-H78-20261026-70（AppCore integration lint仍被既有项阻断）：保留warnings执行`cargo clippy -p deepagent-app-core --test kernel_v2_e2e --offline -j 1 --no-deps`仍退出1：lib原10个warning外，既有mcp_discovery_restore.rs:433在first.is_err分支first.unwrap触发默认deny的panicking_unwrap；另responses_custom_tool.rs:58一个既有bool_assert_comparison warning。对照HEAD确认上述失败代码及base_registry_request八参数签名均原已存在；本轮path_boundary修改未产生诊断，但不能将整体退出1写成lint通过，也未放宽lint/改无关源文件。当前双crate严格all-features lint的退出0仍见E-66。测试helper参数收敛仅改变夹具表达式，最后path_boundary已重新启动实际执行；未获得终态前不标最终通过。

E-H78-20261026-71（最终快照结果及Unix symlink兼容）：测试helper收敛后的Windows path_boundary退出0，8 passed/0 failed/0 ignored（编译1m22s、测试11.22s），随后move_failure退出0，2 passed（1.51s）；最终fmt --check退出0。四项builtins ignored经实际测试二进制--ignored --list确认：本轮跨卷/计时/UNC三项与既有bash drain探针，后者本项未执行。E-56 UNC命令另按实际字符核对为2个前导反斜杠、全路径共4个，内容正确，无需重写历史记录。为验证共享解析器的Unix静态symlink兼容，新增cfg(unix)默认测试三项：四种FsAccess下内/外向及先外后内隐藏链、敏感中间目标、glob/grep与目录循环；root alias/新建Unicode父路径及file symlink末项替换保留target正文；clone共用原dev/ino身份，首次读取前同名新根拒绝。沿E-63独立1.96.0环境执行同一Linux builtins lib命令退出0，305 passed/0 failed/0 ignored（编译18.62s、测试5.04s）。这些是静态策略/兼容回归，明确不证明Unix字符串I/O可抵抗操作期间rename；Linux临时toolchain无clippy，未将Windows lint冒充Linux lint。H-78的Windows原问题及本机工具链已具备所列验收证据，下一步整理最终范围、台账和独立提交。

E-H78-20261026-72（修复验收结论，2026-10-26）：H-78按原问题“Windows工作区junction导致文件工具越界”的范围完成本机验收，H-279只归并，不重复计数。根因修复落在既有WorkspaceRoot的逐跳物理授权、创建时根File身份与Windows实际I/O边界：native no-reparse打开/同句柄读取枚举，写父链pin/guard及句柄发布，delete/move保留entry语义并复用H-76版本提交和H-286 staging/rollback；主RunAssembler传入实际FsAccess，与hook一致。没有新增运行/事件存储、权限中心或供应商分支，无schema/模型协议/UI DTO迁移。Windows新增根/父目录pin可能在操作期间使外部rename返回sharing violation；不支持可靠File身份的provider明确拒绝，不静默退回不受限路径。修复代码及本轮记录准备独立提交，hash在提交后回填。

| 验收范围 | 实际证据与结果 |
|---|---|
| 原外向读取、隐藏中间目标、内向与Full控制、registry组装后同名根替换 | 最终path_boundary 8 passed/0 ignored，含16组mutation磁盘+事件+下一wire验证；E-68/71 |
| Windows文件操作、目录分页/Unicode、父链替换/reparse、版本/取消/staging/回滚及末项file link | builtins默认349 passed/4 ignored；相关默认回归均实际执行，失败历史保留；E-65 |
| 版本及move正式消费者 | file_version 3 passed，move_failure 2 passed；E-69/71 |
| SUBST与本机WSL UNC | 显式namespace公开工具两项通过，含UNC根身份替换拒绝；E-60，不外推任意SMB服务 |
| C/G两个独立NTFS卷 | 显式跨卷1项通过，文件/目录×缺失/覆盖四组失败保留/回滚均断言；E-64 |
| Unix共享解析器兼容 | Linux305 passed，含3项真实symlink/根身份/发布语义回归；persistence68+9 passed，doc-tests0；E-64/65/71 |
| lint与格式 | builtins/persistence all-targets/all-features严格lint退出0；fmt/diff检查退出0；AppCore严格及普通lint被既有项阻断，见E-69/70 |
| 完整workspace | E-55真实执行后因既有H-306失败，后续未执行项不算通过；不能宣称全仓绿灯 |

验收限制：本项不承诺Unix操作期抗rename，Unix仍保留字符串I/O；macOS环境、真实UI/供应商及断电未验证；本机UNC仅WSL provider，不保证所有远端文件系统语义。move多次rename不具备断电事务原子性，版本检查也不是对非协作写入者的文件系统CAS；H-287 DACL等独立问题仍保留。此次统一的是文件工具边界，没有将CheckpointManager等宿主自主I/O全部迁入WorkspaceRoot：静态核对capture_before仍先走词法confined并在工具执行前备份，未动态证明其物理授权，不能据此宣称全系统所有文件I/O已受同样保护；后续需单独复核，不新增未复现的缺陷编号。计时E-61仅当前Full/Workspace热读取抽样，非旧版本对比或性能承诺。以上限制保持明示，不改写原失败或关闭其他H编号。

## H-44 本轮即时记录与验收（2026-10-26）

**当前状态：已修复并验收，限下述Windows本机范围；修复提交`3d247d1a`。** 以下为逐步追加的历史状态，保留首轮失败与中间结果，不将中间“尚待”改写为当时已完成；本次仅另做hash回填，不amend、不推送。

E-H44-20261026-01：HEAD `75a1b3e6`执行`cargo test -p deepagent-app-core --test recovery_terminal_audit audit_startup_recovery_preserves_one_replayable_terminal --offline -j 1 -- --exact --ignored --nocapture`退出1，0 passed/1 failed（0.34s）。正常恢复及SQL故障后重试/重开均run_kind=failed但run_terminals/session_terminals=0、conversation_turn_kind=null；故障重试recovery_markers从1变2。H-32修复不覆盖本根因；证据/调用路径即时写入原项，尚未修改生产代码，需统一终态边界、跨存储补偿、按run归属且可重试。H-95等独立问题不归并；仅本地合成数据，无网络/用户数据。

实施前边界核对：会话终态增加可选 `run_id` / `task_id`，旧JSON默认无身份仍可读取，现有Tauri/SDK DTO不变；新增身份终态只按身份定位，不能回退污染最新turn。正常kernel、准备失败、workflow及启动恢复共用RunStore规范提交与EventStore文件投影。恢复标记与规范终态同一SQL事务；现有run_events中的规范终态作为待投影来源，文件落盘后追加幂等`session_terminal_projected`确认（本地设计，不宣称参考项目使用相同存储协议），无需schema迁移/第二套store。投影在既有session I/O与OS文件锁内读取权威文件去重，不能只看SQLite游标；恢复候选包含未确认投影及历史仅finish但带恢复标记的终态。历史无身份终态仅在明确task区间内复用；缺失/冲突不猜归属。原有重复恢复标记不改写历史，新事务不再增加重复。尚未实施或通过验收，H-95独立保留。

E-H44-20261026-02：首版`cargo test -p deepagent-app-core --test recovery_terminal_audit audit_startup_recovery_preserves_one_replayable_terminal --offline -j 1 -- --exact --nocapture`退出0，1 passed/0 ignored（测试0.34s，编译1m26s）。两组均markers/run_terminals/session_terminals各1、conversation_turn_kind=failed，原探针已转默认；不再使用`--ignored`伪称执行。此结果只验首版原场景，文件/确认故障、多run、旧格式、正常kernel/workflow与全组/lint仍待，H-44不关闭。

E-H44-20261026-03：追加7项后`cargo test -p deepagent-app-core --test recovery_terminal_audit --offline -j 1 -- --nocapture`退出0，8 passed/1 ignored（0.56s，编译1m22s），唯一ignored仍是独立H-95。实际临时文件暂不可用后规范SQL终态保留并由重开补齐；文件已写但cursor/ack的SQL trigger拒绝，分别经同进程竞争收尾和重开均无重复、且不覆盖先提交成功；多遗留run及后来成功turn归属、历史重复恢复marker不改写/只补规范终态、旧无身份terminal复用、冲突/缺task拒绝均通过。首次`cargo fmt --all`遇os error1224未成功，尚待重试；新增并发/活跃owner及正常链路强化验证、全组/lint仍待，不宣称最终验收。

中间失败实录：追加startup-ack/并发/活跃owner测试后，组合`cargo test -p deepagent-app-core --test recovery_terminal_audit --test owned_recovery --test host_lock --test host_session_audit --offline -j 1 -- --nocapture --test-threads=4`退出1，新增集成测试错误调用crate私有`take_admission`（E0624），测试体未执行，不算回归通过。保留私有边界，改用公开admit持有coordinator、drop释放（已有owned_recovery相同用法），不为测试扩大生产API。此前fmt/diff重试退出0；本次还将正常kernel、preflight和workflow终态断言补强，须重新实测。

E-H44-20261026-04：上项修正后组合命令另加`--test workflow_audit`退出0：host_lock2/0（1.20s）、host_session5/0（3.56s）、owned_recovery7/0（5.15s）、recovery_terminal11/1（0.75s）、workflow5/41（0.40s）。startup自身ack失败重开补偿、独立连接并发文件去重、pending投影受活跃owner阻挡及正常workflow新增断言通过；其他ignored不计通过。后续兼容性核对发现，历史rewind可合法移除task但保留run ledger，不能将这种缺task一律当损坏阻断启动：改为只保存带身份的历史终态、不重建消息/turn；冲突或格式非法身份仍拒绝。recycle则保留待投影状态，用户明确restore后再补，不自动复活。新增两项正式历史控制回归待跑，故E-04不是最终源码验收；原E-03的拒绝缺task为首版当时结果，保留不改写。

跨层补核：fork正式原语保留payload/task但不复制源run表；新增keyed terminal不能因子session无源run记录而丢弃，也不能将源run当作子session执行所有权。消费者改按显式task定位其既有`child_session:task`历史turn，若存在本地run/task绑定则必须匹配。增加正式AppService fork/继续子会话对照，验证状态保留、稳定重读、不克隆源run、不改源会话。五crate整组当前正在运行；此项追加晚于该命令启动，因此该轮即使绿色也不冒称最终冻结源码验收，后续需重新确认。

E-H44-20261026-05：五crate组合`cargo test -p deepagent-app-core -p deepagent-core -p deepagent-persistence -p deepagent-session -p deepagent-runtime --offline -j 1 -- --quiet --test-threads=4`退出1（编译6m05s、AppCore17.45s）：AppCore1130 passed/2 failed/69 ignored，后续targets未执行。两项run_coordinator取消回归的旧精确事件断言仍只期望4条，本轮新增文件投影确认后实际5条，差异恰为`session_terminal_projected`；调整为完整5条顺序精确校验，并加强文件中对应cancelled终态仍恰好一条，不过滤/放宽成“至少包含”。这次是本轮变更需要同步的测试契约，不归咎既有基线或环境。后续冻结源码重新跑整组/workspace。

E-H44-20261026-06：最终源码首轮`cargo test --workspace --offline -j 1 -- --quiet --test-threads=4`退出1（编译8m48s）：AppCore1132 passed/78 ignored（17.03s），此前两项取消回归已通过；checkpoint18/1等先行targets通过，但golden_trace为1 passed/2 failed（0.36s），旧fixture及tool测试仍要求run_terminal为最后一条，实际其后新增session_terminal_projected。后续recovery/runtime等targets未执行，不计通过；须先核对正式事件消费者与协议，再同步完整契约，不能过滤确认事件掩盖差异。严格`cargo clippy --all-targets --all-features --offline -j 1 -- -D warnings`退出1，在未修改的deepagent-models/src/client.rs:355命中clone_on_copy；不修改无关源码或声称全库lint通过。两命令均已结束，H-44仍未验收提交。

契约核对与测试同步（尚待实跑）：Desktop取消判定按event_type查run_terminal，stdio turn/stream原样序列化各run ledger并保留sequence，协议Runtime事件使用开放字符串，未发现正式生产消费者要求canonical必须为最后一条；旧假设位于golden测试。明确新增v1投影确认是终态之后的ledger元数据，不是第二个outcome，也不隐藏或重编号。同步完整simple fixture及tool尾部精确两条，强化所有golden场景的唯一终态/唯一确认、连续sequence、terminal游标之后仍可补读确认、确认关联真实会话event id/sequence及run/task/kind/reason断言；外部依赖last()判定结果的消费者须改按event_type。当前只改测试与Rust契约注释，尚不记验收。新增格式检查发现一处数组需换行（退出1），后续格式化再验，不抹去失败。

中间夹具编译失败：golden/recovery组合命令在golden_trace.rs:88报E0308，新增断言误把&StoredRunEvent再次取引用，两个target均未执行。改为比较Option<&StoredRunEvent>，不改生产代码或放宽断言；待格式化后重跑。

E-H44-20261026-07：修正夹具并`cargo fmt --all`退出0后，`cargo test -p deepagent-app-core --test golden_trace --test recovery_terminal_audit --offline -j 1 -- --nocapture --test-threads=4`退出0：golden3 passed/0 ignored（0.27s）、recovery14 passed/1 ignored（0.95s）；fork/recycle/rewind三项最终补强均通过，原探针两组marker/canonical/session各1、turn=failed，H-95仍独立ignored。确认关联文件身份及terminal游标续读均实测通过；随后fmt-check与diff-check退出0。全库第二轮及相关lint待结果，仍不将本机专项当整体通过。

E-H44-20261026-08（lint阶段）：`cargo clippy -p deepagent-core -p deepagent-persistence -p deepagent-session -p deepagent-runtime -p deepagent-app-core --lib --offline --no-deps -j 1 -- -D warnings`退出1，core/persistence/session检查后在未修改的runtime/model_agent.rs:225遇clone_on_copy。未增加无关allow/修复；普通相关库lint及专项测试lint另行验证，不替代严格全库结果。

最终验收边界核对（待全组结果）：本轮无schema迁移、新store或新运行链，不改DeepSeek参数、审批/沙箱、Tauri命令/视图DTO或前端。旧session JSON可读，新增可选字段对手写Rust枚举构造方是源码契约变化，仓库构造方已同步；run ledger新增v1确认，外部消费者不可依赖last()是outcome。只补带旧recovery marker的finish-only历史形态，不声称修复任意损坏/未知终态。文件与SQLite非跨介质原子事务，确认前失败需显式重试或后续Primary恢复；busy会话跳过、recycle须用户restore；强杀/断电、磁盘写满、跨平台/网络FS、真实供应商/Tauri手动交互未验。按code-audit人工追链与临时SQL/文件故障、docs-generator即时证据链记录；参考Codex仅借鉴idle时将InProgress turn按身份投影Interrupted，未运行参考测试，也不声称同一存储协议。

E-H44-20261026-08补验：同五crate普通lib no-deps clippy（上条去掉`-- -D warnings`）退出0；runtime既有1/AppCore既有10 warnings，无本轮新增生产告警（run_assembler告警为未改的base_registry_request）。`cargo clippy -p deepagent-app-core --test golden_trace --test recovery_terminal_audit --test workflow_audit --offline --no-deps -j 1`退出0；golden/recovery无新增告警，workflow有既有media.rs:148 redundant_locals及旧段manual_contains共3项。普通lint完成不替代严格/all-features结果，也未修改无关行或新增豁免。

E-H44-20261026-09：第二轮冻结源码`cargo test --workspace --offline -j 1 -- --quiet --test-threads=4`编译6m52s后退出1。AppCore1132/78（18.18s）、golden3/0、host_lock2/0、host_session5/0、owned_recovery7/0、recovery14/1、workflow5/41、checkpoint18/1、kernel E2E107/90、core41、persistence67/session_control9、CLI88/27、protocol12、SDK5/12+stdio6、builtins312/1及models96/25均无失败（斜线后为ignored，不计通过）。到prompts时既有H-306再次27 passed/1 failed，canvas_prompt.rs:310的creative.video-edit.v1.defaultUserPrompt为空，完整workspace未通过；后续runtime/session及全库doc-tests未执行，须补跑相关targets，不将未执行项算通过。H-44新增回归没有失败，不扩修H-306。

E-H44-20261026-10：`cargo test -p deepagent-runtime -p deepagent-session --offline -j 1 -- --quiet --test-threads=4`退出0（编译54.84s）：runtime284 passed/16 ignored、stabilization5 passed（57.04s）、tokenizer1 passed/1 ignored、schedule0/2，session14 passed，两个crate doc-tests均0。`cargo test -p deepagent-core -p deepagent-persistence -p deepagent-app-core --doc --offline -j 1 -- --quiet`退出0，三crate各0文档测试，不冒称执行了文档行为样例。最后fmt-check、diff-check退出0。至此相关生产边界及全部本轮默认回归已实际验证；完整workspace与严格lint仍有E-06/E-08/E-09所列失败。

验收结论（2026-10-26）：H-44在上述Windows本机、临时SQLite/文件与离线transport范围已修复并验收；F-H44修复依据E-01原失败及E-07～10最终专项/跨层结果。P-H44-repair（callflow）：现有RunStore统一SQL终态事务 → 既有EventStore按权威文件及run/task身份幂等投影 → run_events v1确认 → Primary持有session owner后补偿未确认记录；每步由上述故障/重开/正常链路回归覆盖。不是恢复模型自动续跑，也不保证任意断电或所有外部副作用停止。无生产数据迁移、不推送；本轮15个相关文件待独立修复提交，hash随后另行回填。本阶段新增1项、累计11已验收/17历史跳过/7归并/2撤销/269有效待修，整体goal仍active。下一项H-78未启动。剩余200–400有效小时仍为未独立计量校准的粗估，环境/构建等待另计，不机械扣减。

交付核对：修复提交`3d247d1a`仅含本项15个文件（代码、测试及当时两份文档），`cargo fmt --all -- --check`、`git diff --check`、`git diff --cached --check`均退出0。台账Rows=306、Unique=306、Missing=[]、BadColumns=0，11已验收/17历史跳过/7归并/2撤销/269待修/修复中0。所有本轮测试/lint进程已结束；原有五类未跟踪项未读、未改、未暂存。当前仅另做hash回填，不amend、不推送；下一项H-78尚未复现或修改，整体goal未完成。

## H-32 本轮实施与验收明细（2026-10-26）

**当前状态：已修复并验收，限原项列明的Windows本机范围。** 以下按执行顺序保留中间状态及失败，不把中间结果当最终验收；修复提交 `c015bc8f`，本次另做hash回填，不amend、不推送。

H-32即时记录（2026-10-26）：HEAD `ce1e734a` 原探针 `cargo test -p deepagent-app-core --test host_lock audit_standby_open_preserves_live_actions_and_subagents --offline -j 1 -- --exact --ignored --nocapture` 退出1，0 passed/1 failed，0.71s（E-H32-20261026-01）。取回已有进程结果，未重复启动。Standby打开使父action Failed、child cancelled，正常完成分别IllegalTransition/NotFound；父run仍running。证据与调用路径即时写入原项；尚未改生产代码，需复用session dispatch边界保护活跃主/备用/同进程owner并保留真正崩溃恢复，不把搬到Primary当作完整方案。

H-32首版追加：Database::open移除运行恢复；EventStore持有activity/dispatch scope授权按session恢复，锁内重新读run；候选包含已终态父run遗留动作/审批/child，child按当前父run而非origin；执行租约仅按到期UPDATE。原公开全量recover/cancel_orphaned入口删除，AppService统一使用scope，不新增运行/store/取消中心。E-H32-20261026-02：`cargo test -p deepagent-app-core --test host_lock --offline -j 1 -- --nocapture` 退出0，2 passed/0 ignored，1.20s，原探针默认执行且原owner成功写入。新增owned_recovery场景尚未跑；首次fmt os error1224失败，重试退出0；完整回归/lint未验，不据2项绿色关闭。

H-32专项实测追加（E-H32-20261026-03）：`cargo test -p deepagent-app-core --test owned_recovery --offline -j 1 -- --nocapture` 5 passed/0 ignored（0.21s）；加入锁错误/24轮双线程竞争/子表部分失败后重跑7 passed/0 ignored（5.18s）。覆盖活跃Standby保留及kill/wait后恢复、Primary重复open/from_shared、终态父run孤儿控制、当前父系、scope对接纳/历史改写/重复恢复的互斥与锁内重读；真实SQLite失败回滚/部分提交重试无重复action.failed、guard释放、到期/有效terminal lease对照。模型/工具未真实执行；workspace/lint尚未验收，状态仍修复中。

H-32兼容性/残余：CLI/Desktop现有AppService恢复调用已静态核对；无schema/DTO/DeepSeek/权限变更。旧公开无owner的Rust恢复API收窄，仓库调用方同步，外部直接依赖方需迁移至SessionRecovery；协作宿主须全部升级。忙session整组跳过，旧记录需owner退出后再次恢复/重启，不新增周期重试或Standby自动晋升；OS锁不隔离非协作写者，不证明脱离Future的外部副作用停止。Linux/macOS/网络文件系统、真实工具/供应商/Tauri UI未验；Prepared/Blocked和未到期审批原语义保留，H-44/其他取消问题仍独立。按code-audit逐生产方/消费方核查、docs-generator即时证据链落文档，未复制参考业务代码。

H-32整库首轮：`cargo test --workspace --offline -j 1 -- --quiet` 编译7m01s，AppCore lib1130 passed/1 failed/78 ignored（16.23s），既有`completion_plan/audit_tests.rs:42`等待build-started失败，退出1，后续targets未执行；不覆盖该失败，定向复验/补跑待结果。严格两crate lib no-deps lint退出1，既有`run_store.rs:127 explicit_counter_loop`阻断（未改该段）；不屏蔽告警。fmt/diff检查退出0；306行/唯一306/无缺号/8列检查通过。尚无最终整项验收。

H-32后续补验：上述completion_plan测试以`cargo test -p deepagent-app-core --lib completion_plan::audit_tests::discovered_real_build_completes_after_release --offline -j 1 -- --exact --nocapture`单独执行1 passed/0 ignored（1.42s）；未改测试、未进一步归因，不抹掉首轮失败。两crate普通lib no-deps clippy退出0（persistence既有1/AppCore既有10 warning，无本项新增生产告警）。新竞争测试释放改为有界channel避免故障等待永久挂起；fmt退出0，最终源码workspace用`--test-threads=4`重跑及host_lock/owned_recovery定向lint尚在执行。

H-32最终源码workspace：`cargo test --workspace --offline -j 1 -- --quiet --test-threads=4` 编译32.40s；AppCore1131/78、host_lock2/0、host_session5/0、owned_recovery7/0（5.13s）、checkpoint18/1、E2E107/90、workflow4/41，persistence67/session_control9，CLI88/27、builtins312/1、protocol12、SDK5/12+stdio6、models96/25通过。prompts仍因既有H-306空defaultUserPrompt为27 passed/1 failed，完整命令退出1；后续runtime/session及全库doc-tests未执行，相关补跑待结果。`cargo clippy -p deepagent-app-core --test owned_recovery --test host_lock --offline -j 1 --no-deps`退出0，无新增测试告警（保留生产既有告警）。全库all-features lint/跨平台/UI未验，H-44未关闭；尚待最终补验和提交。

H-32最终补验（E-H32-20261026-04）：`cargo test -p deepagent-runtime -p deepagent-session --offline -j 1 -- --quiet --test-threads=4`退出0，runtime284/16、stabilization5（56.27s）、tokenizer1/1、schedule0/2，session14，两crate doc-tests0。`cargo test -p deepagent-app-core -p deepagent-persistence --doc --offline -j 1 -- --quiet`退出0，各0 doc-tests，不冒称执行文档行为样例。完整workspace/严格lint仍有上述既有失败；其他未执行targets/doc-tests不记通过。E-01原失败、E-02修后原探针与E-03/最终workspace专项，结合正式调用链核对，验收H-32所列协作式恢复所有权根因，不关闭H-44或其他取消/外部副作用问题。

本轮新增1项、累计10已修复/17历史跳过/7归并/2撤销/270有效待修，整体goal仍active。新scope只复用既有锁/存储，源码API及多宿主升级注意事项见上；无schema迁移/DTO或模型配置变化。最终fmt/diff/306行台账及显式暂存范围核对后提交，hash另回填；用户原有五类未跟踪项不读、不改、不暂存。

交付核对：`cargo fmt --all -- --check`、`git diff --check`、显式暂存后`git diff --cached --check`均退出0；Rows=306/Unique=306/Missing=[]/BadColumns=0，10已修复/17历史跳过/7归并/2撤销/270待修/修复中0。修复提交 `c015bc8f`只含本项12文件；原有5类未跟踪项未读、未改、未暂存；本轮所有测试/lint进程已结束。当前仅文档hash回填；下一项H-44未启动，不把本项恢复所有权当作其终态投影修复。整体goal未完成。

## H-33 本轮实施与验收明细（2026-10-26）

**当前状态：已修复并验收，限原报告列明的Windows本机范围。** 以下按执行顺序保留中间状态，不把中间绿色当作最终源码验收。代码/测试与两份文档已一起独立提交 `8ed5d82e`；本次另做文档回填，不amend、不推送。

H-33即时记录（2026-10-26）：原双进程探针先实测失败（E-H33-20261026-01，0 passed/1 failed，0.47s），再接入既有EventStore独占dispatch scope与RunCoordinator admission/outer registration；本地队列共享owner，输入租约移至outer生命周期，等待Future丢弃清理队列。首轮 `cargo test -p deepagent-app-core --test host_session_audit --offline -j 1 -- --nocapture` 为1 passed/0 ignored（3.44s，E-H33-20261026-02）：竞争宿主无模型派发、无run记录，父正常完成后可续聊。新增的异session并行、本地队列打断、失败/Future abort、真实进程退出接管尚在验证，不据首轮绿色关闭H-33。H-32及崩溃run终态恢复仍独立；没有改schema/供应商/前端/权限边界。

追加实测：新增四个正式场景后同命令为5 passed/0 ignored（3.49s，E-H33-20261026-03），异session并行、本地排队打断与两run终态、准备失败/Future abort后接管、真实持锁进程kill/wait后接管均通过；崩溃旧run仍unfinished，不能声称恢复已修。底层、AppCore整组和workspace/lint尚未完成，H-33仍修复中。

中间版本 `cargo test -p deepagent-app-core -p deepagent-persistence --offline -j 1 -- --quiet` 退出0（AppCore lib 1130/69、E2E107/90、checkpoint18/1、host_session5/0；persistence67、session_control9），coordinator专项10 passed。自查发现“恢复失败时本地lease仍在inner结束”的交接窗口，现已改成在registry登记时即归outer registration，两个正式入口同步，并补缺失session恢复失败回归；前述结果明确属于中间版本，完善后workspace/专项待重验，不提前关闭。

完善后 `cargo test --workspace --offline -j 1 -- --quiet` 已完成编译（7m00s），AppCore lib1131/78、host_session5/0、E2E107/90、checkpoint18/1、workflow4/41已通过，新恢复失败/租约析构回归已在默认lib集中运行；其余crate及lint仍在执行，完整workspace暂无通过结论。

workspace最终退出1：既有H-306（prompts27 passed/1 failed，`creative.video-edit.v1`空defaultUserPrompt）；此前persistence67/session_control9、CLI88/27、protocol12、SDK5/12+stdio6、builtins312/1已通过。后续targets/doc-tests未执行，runtime/session另补跑。两crate严格lib no-deps lint被未改的`run_store.rs:127 explicit_counter_loop`阻断；普通lint尚未结束。完整workspace与严格lint不记通过。

最终验收：`cargo test -p deepagent-runtime -p deepagent-session --offline -j 1 -- --quiet` 退出0（runtime284/16、stabilization5、tokenizer1/1、schedule0/2，session14，两crate doc-tests0）。两crate普通lib no-deps clippy退出0（AppCore既有10warning、persistence既有1warning）；all-targets普通no-deps lint因既有`mcp_discovery_restore.rs:433 panicking_unwrap`退出1。两个新增/修改集成测试分别用`cargo clippy -p deepagent-app-core --test host_session_audit --offline -j 1 --no-deps`及`cargo clippy -p deepagent-persistence --test session_control --offline -j 1 --no-deps`检查，均退出0且无新增测试告警。完整workspace/严格lint仍失败；未屏蔽告警，相关失败路径git diff确认未改。最终fmt/diff与306行台账唯一性/无缺号/列数检查通过。

实现/兼容性：独占dispatch锁与既有shared activity/history control锁职责分开，admit前取得、outer收尾后释放，同coordinator队列共享Arc/Weak索引；input lease在登记时即交outer并处理被丢弃等待项。复用既有内核、运行/事件存储、审批/取消；无schema/供应商/DeepSeek协议/前端DTO变更。参考Codex writer-before-recovery及guard生命周期，队列共享是适配本内核的本地设计。协作宿主须全部升级；无跨宿主取消转发，不保证Future abort后全部外部副作用静止；崩溃旧run终态仍由独立H-32/H-44处理，未归并关闭。Linux/macOS、网络文件系统、真实UI/供应商与副作用验收未跑。按code-audit人工核对调用链并按docs-generator即时证据链记录；本项修复提交 `8ed5d82e`，此次仅文档回填，整体goal未完成。

## 总体工时、排序与不确定性

暂估剩余 **200–400 小时有效修复、测试和文档交付时间**，是基于当前有效未关闭规模、跨层 owner/授权/持久化问题与多数单项需原输入回归的初始规划区间，不是将每项假定为相同难度。已交付项复杂度不同，且尚无独立计量的有效工时，不能据此虚构可靠吞吐或机械扣减工时；继续按实际样本与剩余根因分组滚动校准，不编造精确结束日期。不以对话跨度或编译等待伪造工时样本。

设备、其他 OS/SDK、真实供应商或 UI 环境的等待时间 **尚无法量化，另计，不混入有效工时承诺**。不可取得的验收必须按具体问题报告阻塞或限制，不静默关闭。优先队列按原报告 A/B 组：文件损坏/授权一致性 → 未知结果副作用 → 接纳/审批/持久提交 → provider/计量/画布 → 历史/缓存/客户端。H-78 Windows本机验收已完成，真实跨卷与Linux共享解析兼容已执行，矩阵及未验证边界见E-72；workspace仍因既有H-306失败，不能写成全仓通过。当前8层路径公开read计时E-61仅本机小样本。H-78独立提交后，下一项准备复核H-77的原敏感文件搜索探针，先检查共享边界改动是否已使原复现不成立，不提前关闭或新增重复生产修复。H-32所有权与H-44终态投影分别验收、不归并。H-238/H-260/H-287等仍独立待修。

## H-286 本轮实施与验收明细

- 原问题：授权 overwrite 移动到源的祖先目录时，先递归删除目标，连源和无关 sibling 一起删除，再 rename 失败。
- 范围：deepagent-builtins 原 MovePathTool 的私有 native move seam、回归测试和正式 AppCore 原探针；不改其他 H 项，不新建权限/检查点/运行存储，不改前端/协议 DTO，不操作用户目录。
- 根因修复：先以实际路径身份检查同路径、workspace root 及祖先/后代关系；旧目标先 rename 到专属 sibling staging，源发布失败则恢复；所有发布/恢复使用 no-replace 原语，避免覆盖期间新出现的目标。恢复失败保留旧目标且返回 recovery_path/restore_error；发布已成功但清理失败仍准确报告 moved/verified 加 warning，避免诱导重试。staging 在容纳业务数据前取消自动析构删除，panic 不会自动删掉唯一旧副本。
- 实现边界：同一 blocking 本地操作包含 staging/发布/恢复，丢弃异步等待不在两次 await 间隐式清理旧数据；这不等于停止已启动的 native 调用，也不等于 crash-atomic/断电事务。Windows MoveFileExW 不启用 REPLACE_EXISTING/COPY_ALLOWED；Linux/Android renameat2 NOREPLACE、macOS renamex_np EXCL；不支持该能力的系统/文件系统显式失败，不降级为先删后移。未增加依赖，保留 ToolPipeline 权限、检查点和结果通道。

| 实际命令 | 结果 | 证明范围 / 限制 |
|---|---|---|
| `cargo test -p deepagent-app-core --test kernel_v2_e2e move_failure:: --offline -- --include-ignored --nocapture`（修复前） | 1 passed / 1 failed | 原始正式祖先覆盖输入仍破坏临时 source/sibling；显式检查点恢复成功 |
| `cargo test -p deepagent-builtins --lib file_move:: --offline -- --nocapture`（首次修复后） | 11 passed | 文件/非空目录替换、关系拒绝、stage/publish/rollback/cleanup/panic/竞争目标；仅此时版本，后续加的正常竞争回归另验 |
| `cargo test -p deepagent-app-core --test kernel_v2_e2e move_failure:: --offline -- --nocapture` | 2 passed / 0 ignored | 原故障探针已移除 ignore，正式模型工具链、下一 wire、文件 SQLite 重开与检查点恢复通过 |
| `cargo test -p deepagent-builtins --offline`（补最后一条竞争回归前） | 291 passed / 1 ignored；doc-tests 0 | 包含真实 Windows source 占用导致 native 发布失败、恢复旧目标的默认 Tool 回归；ignored 是原有其他 H 探针，不作通过 |
| `cargo test -p deepagent-builtins --offline -j 1 -- --quiet`（最终版本） | 292 passed / 0 failed / 1 ignored；doc-tests 0 | 包含最后追加的“absence 检查后出现新目标”正常拒绝回归，全部 default builtins 测试通过；不执行其他问题 ignored 探针 |
| `cargo test -p deepagent-app-core --test kernel_v2_e2e --offline -j 1 -- --quiet` | 102 passed / 0 failed / 92 ignored | 正式 AppCore 工具链整组默认回归；本项原探针默认执行，不以 ignored 冒充通过 |
| `cargo clippy -p deepagent-builtins --all-targets --offline -- -D warnings` | 未通过 | 未修改的 models/client.rs:355 clone_on_copy 阻断依赖 lint，不伪称本轮引入或顺手修复 |
| `cargo clippy -p deepagent-builtins --all-targets --offline --no-deps -- -D warnings` | 退出 0 | 本问题 crate 全部 target 严格 lint；不替代全依赖 lint |
| `cargo test --workspace --offline -- --quiet` | 未通过 | 默认并行编译内存分配失败/编译进程异常退出，不能列为功能通过 |
| `cargo test --workspace --offline -j 1 -- --quiet` | 编译完成；AppCore lib 1100 passed / 1 failed / 80 ignored，退出 1 | 既有 `completion_plan::audit_tests::discovered_real_build_completes_after_release` 的临时 Cargo 构建未在 4 秒内创建启动标记；Cargo 早停，其他未运行 target 不算通过 |
| `cargo test -p deepagent-app-core --lib completion_plan::audit_tests::discovered_real_build_completes_after_release --offline -j 1 -- --exact --nocapture` | 1 passed，1.18 秒 | 独立复测正常启动/收尾；该夹具注册表为空，不经过 MovePathTool。具体调度/资源成因未进一步确认，不把独立绿色覆盖前述全量失败 |

本轮未执行 Linux/macOS/Android 动态或交叉编译、真实跨卷故障、掉电/崩溃恢复和真实 Desktop UI；主机仅安装 Windows 与 Android Rust targets，不能由 Windows 测试推断这些平台通过。case-alias 已在 Windows 测试；Unix symlink-parent 回归存在但本机不执行。没有修改 SandboxBackend、数据库 schema 或公共 command/DTO。

`cargo fmt --all -- --check`、`git diff --cached --check` 与 306 行台账唯一性/集合/列数校验通过。最终源码移除重复前置判断后，重新运行 builtins/AppCore 两组测试，仍分别为 292 passed/1 ignored、102 passed/92 ignored；格式与 no-deps 严格 lint 再次通过。新增文档首次暂存检查发现一个 EOF 空行，移除后暂存检查通过。代码、测试及两份文档已作为本项独立提交 `56b9e7b9`；本次仅补文档记录，不反复 amend。H-286 标记为“已修复并验收”限上述 Windows 本机正式路径；完整 workspace、全依赖 lint 和其他平台没有通过状态，整个修复任务仍未完成。文档技能的证据链要求用于区分原失败、本机验收及未验范围；人工核对归并/撤销，不把历史证据冒充本轮重验。

## H-72 本轮实施与验收明细

H-72 当前实施记录（2026-10-26）：修复前执行 `cargo test -p deepagent-app-core --test checkpoint_audit audit_missing_backup_preserves_current_file_on_failed_rewind_and_fork --offline -j 1 -- --include-ignored --exact --nocapture`，**0 passed / 1 failed**，测试体 0.16 秒。rewind/fork 均为 failed=true、current_file_exists=false、healthy_target=second-before、history_unchanged=true、checkpoint_unchanged=true、session_count=1。仅使用原临时 fixture；不以本次重验当作新发现。

恢复实现现位于既有 CheckpointManager 的私有 restore 模块：解析全部 checkpoint 后先构建最终替换树，重叠路径保持原恢复顺序；当前工作副本以 no-replace rename 保留到历史提交成功，普通失败则反向补偿，补偿失败保留 recovery 目录并报错。no-replace 原语从 H-286 私有模块提至现有 persistence 共享模块，两处共用，不复制平台分支。AppService 两入口接入同一准备/发布/提交边界；fork 的完整前缀先暂存，在同一 SQLite 事务中写投影后发布，COMMIT 失败撤下派生分支；rewind 以原有文件代际发布为事实，避免发布后 reload 失败被误当作未提交。没有迁移生产数据库或修改公共 DTO。

即时验收时间线（保留中间失败，不代表最终版本反复失败）：原 H-72 探针修复后 **1 passed**，两入口当前两文件均为 after，历史与 checkpoint 未改。恢复层首批 **7 passed**；公开 AppCore `checkpoint_audit` **8 passed / 2 ignored**，覆盖完整备份、缺失旧 checkpoint、文件 SQLite 重开/重试、fork INSERT 与延迟 FK 的真实 COMMIT 失败及无残留子会话。两个 ignored 是未独立编号的 capture 同对象重试契约探针及 H-73，不作本项通过，也不误记为 H-65。首次恢复单测暴露空 suffix 被拼成文件尾斜杠，已改正后原 5 项全部通过。一条历史目录阻断测试实际命中 public preflight，已改名避免假称晚期补偿，并另补私有共享提交 seam 的发布后故障用例。

最终本机结果如下；没有把局部绿色写成 workspace 全绿：

| 实际命令 | 结果 | 覆盖与限制 |
|---|---|---|
| `cargo test -p deepagent-runtime --lib checkpoint:: --offline -j 1 -- --nocapture` | 13 passed / 0 failed / 0 ignored | 原 5 项及新增 8 项；含目录部分复制失败、Windows 占用导致发布失败、完整/非法计划、重叠目录、多检查点、提交失败补偿、补偿失败保留副本与 panic；预期 panic 被 catch_unwind 捕获，不是测试失败 |
| `cargo test -p deepagent-app-core --test checkpoint_audit --offline -j 1 -- --nocapture` | 8 passed / 0 failed / 2 ignored | 原故障默认执行；其他 ignored 不作通过；最终源码也在下一行 workspace 中同结果 |
| `cargo test --workspace --offline -j 1 -- --quiet` | 退出 1，未全绿；编译 6m23s | AppCore lib 1102 passed/80 ignored，kernel_v2_e2e 102 passed/92 ignored，builtins 292 passed/1 ignored，persistence 67 passed；后到 prompts lib 27 passed/1 failed，被既有 H-306 空 `creative.video-edit.v1.defaultUserPrompt` 阻断。之后未运行 target 不算通过；没有修改该问题 |
| `cargo test -p deepagent-runtime -p deepagent-session --offline -j 1 -- --quiet` | 退出 0 | 补跑前述早停未执行模块：runtime lib 282 passed/16 ignored、e2e_stabilization 5 passed、tokenizer 正常项 1 passed/1 ignored、schedule 2 ignored；session 14 passed；两 crate doc-tests 均 0 |
| `cargo test -p deepagent-app-core --lib restore_commit_seam_compensates_files_when_history_rewrite_fails_after_publication --offline -j 1 -- --nocapture` | 1 passed；最终源码另在 workspace AppCore lib 通过 | 共享 private seam 在发布文件后才引入真实 history 故障并验证补偿，不冒称 public 并发故障注入或 SQLite 投影错误 |
| `cargo clippy -p deepagent-persistence -p deepagent-runtime -p deepagent-builtins -p deepagent-app-core --all-targets --offline --no-deps -j 1 -- -D warnings` | 退出 1，未通过 | 未修改的 persistence/run_store.rs:127 `explicit_counter_loop` 阻断 |
| 上条命令尾部追加 `-A clippy::explicit_counter_loop` | 退出 1，未通过 | 继续遇到未修改的 runtime/model_agent.rs:225 `clone_on_copy` |
| 再追加 `-A clippy::clone_on_copy` | 退出 1，未通过 | 继续遇到未修改的 AppCore/plugin_preparation.rs:14 `type_complexity`、run_assembler.rs:1687 `too_many_arguments`；三次后停止叠加豁免，源码没有加入 lint allow，不伪称四 crate 严格 lint 通过 |
| `cargo fmt --all -- --check`、`git diff --check` | 退出 0 | 最终源码格式与差异检查；暂存后仍须检查新文件 |

兼容性与残余：不新增外部版本或生产 schema/DTO，仅将已锁定 tempfile 提为 runtime 正式依赖，把既有 libc/windows-sys 发布原语收敛到 persistence。Windows 已验，Linux/macOS/Android 构建运行、真实跨卷、磁盘写满、强杀/断电与真实 UI 未验。未知平台/不支持 no-replace 的文件系统显式失败；symbolic-link 恢复树、workspace root 及与 checkpoint storage 重叠的恢复目标明确拒绝，不危险降级。文件系统和 SQLite 不是崩溃原子事务；回滚也失败会保留旧当前副本并报告恢复目录，panic 只保证不由析构删除唯一副本，不保证自动完成补偿；fork COMMIT 失败且撤下派生分支也失败时，错误标出保留目录，可能需人工清理/恢复。清理失败只记录 warning，不把已提交恢复假报失败。外部并发/活动运行、分支身份、非法锚点及 Windows DACL 仍分别受 H-75/H-73/H-285/H-287 约束，未在本项关闭。

本项已完成上述 Windows 行为验收，代码、测试和两份文档一起独立提交为 `d08c776c`；本次另做文档 hash 回填，不 amend、不推送。暂存差异检查通过，306 行编号唯一、无缺号、列数正确，状态为已修复 2/历史跳过 17/归并 7/撤销 2/待修复 278。整体修复 goal 仍 active。code-audit 要求人工核对源头和真实故障，docs-generator 要求区分原始失败、共享 seam 和 public 入口证据，避免将前置拒绝或 ignored 算成补偿通过。

## H-73 本轮实施与验收明细（2026-10-26）

当前状态：**已修复并验收，限下述 Windows 本机范围**。以下按执行顺序保留中间失败和阶段性结果，出现“尚未验收”表示当时状态，不覆盖最终范围限制。本轮代码、测试和两份文档已一并独立提交 `d40eef9a`；本次只回填文档，不 amend、不推送。

- 原探针在 HEAD `78fbbba6` 实跑：`cargo test -p deepagent-app-core --test checkpoint_audit branch --offline -j 1 -- --include-ignored --nocapture`，1 passed / 1 failed / 0 ignored，测试体 0.32s；实际文件 old-first、restored_paths=1。原始证据保留，不重复计新发现。
- 首版根因修改在现有 manifest 绑定稳定 EventId；选择恢复时要求当前权威日志同序号仍是同一事件，而非只比较数字或只允许最新 generation。旧 run/checkpoint 继续供审计查询，H-72 文件准备/发布/补偿契约不变；不新增 store、schema migration、UI/协议 DTO。
- 首版 `cargo test -p deepagent-app-core --test checkpoint_audit --offline -j 1 -- --nocapture`：9 passed / 0 failed / 1 ignored，原 H-73 已取消 ignore，实际 current-branch/restored_paths=0；这是新增分支/旧格式回归前的中间结果，尚未完成最终验收。
- 新回归首次编译因 fixture 将临时 FixedClock 借给长期 Session 而报 E0716，测试未执行；已改为命名 clock 绑定后重跑。不把夹具编译错误记作产品缺陷或行为验收。
- 上述修正后同一命令为 16 passed / 0 failed / 1 ignored（测试体 5.55s）；最后新增 source 缺失拒绝/淘汰备份缺失输入之后仍需重跑，当前不关闭 H-73。
- 最终分支专项重跑同一命令：17 passed / 0 failed / 1 ignored，测试体 5.61s，source 缺失拒绝及淘汰备份缺失输入已通过。当前进入 workspace/runtime/lint 验证；不把唯一未处理的 ignored 探针计通过。
- workspace 实跑 `cargo test --workspace --offline -j 1 -- --quiet`：编译 6m56s，AppCore lib 1102 passed/80 ignored、checkpoint_audit 17 passed/1 ignored、kernel_v2_e2e 102 passed/92 ignored、builtins 292 passed/1 ignored、persistence 67 passed，各组 0 failed；到 prompts 时既有 H-306 再次失败（27 passed/1 failed，creative.video-edit.v1.defaultUserPrompt 为空），命令退出 1。后续未运行 target 不算通过；正在补跑 runtime/session，不将局部绿色说成全量成功。
- 补跑 `cargo test -p deepagent-runtime -p deepagent-session --offline -j 1 -- --quiet` 退出 0：runtime lib 283 passed/16 ignored、e2e_stabilization 5 passed、tokenizer 正常项 1 passed/1 ignored、schedule 2 ignored、session 14 passed、两 crate doc-tests 均 0。未将 ignored 或其他未运行 crate 当作通过。
- `cargo test -p deepagent-runtime --lib checkpoint:: --offline -j 1 -- --nocapture`：14 passed / 0 failed / 0 ignored（0.14s），包含本项 source 前置拒绝与 H-72 故障补偿；预期 panic 已 catch，不是失败。严格 `cargo clippy -p deepagent-persistence -p deepagent-runtime -p deepagent-app-core --all-targets --offline --no-deps -j 1 -- -D warnings` 退出 1，未修改的 run_store.rs:127 `explicit_counter_loop` 阻断；不伪称通过，不新增 lint allow。
- 普通 lint（上条不带 `-- -D warnings`）仍退出 1：三个生产库检查后，在未修改的 `tests/kernel_v2_e2e/mcp_discovery_restore.rs:433` 命中默认 deny 的 `panicking_unwrap`；生产库另有既有 run_store/model_agent/context_runtime/plugin_preparation/run_assembler 告警。没有将普通 lint 当作严格通过，没有修改或豁免无关问题；最后仅核对本项 checkpoint_audit target。
- 聚焦检查 `cargo clippy -p deepagent-app-core --test checkpoint_audit --offline --no-deps -j 1` 退出 0，未报告本项修改文件告警，但仍带既有生产库 warnings；只证明该选定 target 完成普通 lint，**不替代严格/all-targets lint**。没有继续叠加豁免。
- 兼容策略已按所列回归验证：未重写 generation 1 可在现有 metadata 事务内补齐全部旧格式检查点（包括保留前缀）身份，保留未知 manifest 字段；历史已重写且缺身份的相关备份明确拒绝自动恢复，不靠时间戳猜归属。即使只是旧版 header/project 重写也会保守拒绝；新格式按 EventId 验证，不受 header 代际改变影响。显式按 checkpoint ID 的恢复入口保留供人工核对，不把它当作自动分支选择。已登记 run 若 source 不存在，manager 构造在创建 checkpoint 目录前失败；无 run 的 detached evidence 不获得已证明的自动恢复资格。
- 范围限制：本项不解决活动 turn 与 rewind 的提交竞争（H-75），也没有引入 fork 对所有祖先文件快照的继承机制；fork 验证是 source 有效分支筛选、复制事件前缀与 child 自有 run/checkpoint 隔离，不能冒称完整祖先快照回放。文件 SQLite 重开不是跨进程强杀/断电验收；其他 OS、真实 Desktop UI、完整特性组合/全依赖 lint 未验。本轮未改前端/桥接 DTO/权限/沙箱，未运行 pnpm build 或生产数据迁移。不改变 H-72 的补偿/人工恢复限制。
- 参考与技能影响：重新读取 Claude 的稳定 messageId 快照定位、Codex 对有效历史应用 rollback 的逻辑，仅借鉴稳定来源/当前有效性，不复制业务代码、不假称运行其测试。冻结 source EventId、原日志旧格式补齐与未知归属拒绝是复用现有 CheckpointStore/EventStore 的本地设计。code-audit 用于人工追溯与故障验证；docs-generator 要求即时分离原失败、中间编译错误、实际通过和未验范围。

证据链：E-H73-01 是修复前 HEAD `78fbbba6` 的原 branch 命令与 old-first/1 path 失败观察；E-H73-02 是最终 checkpoint_audit 命令的 17 passed/1 ignored，以及 checkpoint 专项 14 passed/0 ignored（source_type=command，observed_at=2026-10-26，content_hash/artifact_path=n/a；未另存敏感请求或命令日志）。F-H73（severity=P1，status=validated，confidence=high，location=checkpoint_store.rs::list_for_session_after_sequence）由两条证据确认缺陷与本机修复，不扩为整个系统通过。P-H73（path_type=callflow）：manager 创建冻结源身份 → 既有 manifest 落盘 → 对照当前权威事件筛选 → H-72 恢复事务 → 原 rewind/fork 提交；步骤关联 E-H73-02，原错选路径关联 E-H73-01。范围/授权见本文开头，timeline 为本节逐步追加记录。

最终 `cargo fmt --all -- --check`、`git diff --check`、`git diff --cached --check` 退出 0；台账校验 Rows=306、Unique=306、Missing=[]、BadColumns=0，已修复 3 / 历史跳过 17 / 归并 7 / 撤销 2 / 待修复 277，修复中与阻塞待处理均 0。独立提交 `d40eef9a` 仅含本项 6 个文件；用户原有未跟踪项不读、不改、不暂存。下一项 H-75 仍须原探针复核，整体 goal 保持 active。

## H-75 本轮实施与验收明细（2026-10-26）

当前状态：**已修复并验收，限 Windows 本机后端行为及前端生产回调/构建；真实 Tauri 手动交互未验**。以下为逐步追加的当时记录，不抹去中间失败；代码、测试和当时两份文档已一并独立提交 `8eea7bd5`，本次只回填文档，不 amend、不推送。原探针在 HEAD `1a3c7192` 已复现（1 passed / 1 failed / 0 ignored，0.28s），不重复登记。额外源码核对确认 run_inner 的输入租约先释放，外层仍可能补写 Session RunTerminal，因此仅保护模型请求或内层运行不足。

本地设计首版：通过现有 session 文件锁设施提供共享活动/独占控制 scope；保护附着现有 RunCoordinator admission 与 ActiveRunRegistration 生命周期，不新增输入调度 registry、运行/事件存储或数据库迁移。AppService 在恢复文件前获取独占 scope，原始 truncate 通过同一边界，append 与直连 Kernel/RuntimeEngine 也接入；忙时拒绝，不自动取消。fork 也会恢复同一工作区，故复用同一互斥规则，但不关闭 H-74 等独立问题。

即时验证：`cargo test -p deepagent-persistence --test session_control --offline -j 1 -- --nocapture` 首跑 **6 passed / 1 failed / 0 ignored**。跨 handle、真实子进程共享/独占、强制退出后释放均通过；故障回归在仍保留故意创建的 g2 阻断目录时读取日志而失败（5000ms 打开超时），已改为移除该空临时目录后核对历史，待重跑。即时丢弃 must_use guard 的测试警告已调整，尚未称严格 lint 通过。原失败及中间结果保留，其他平台、真实 UI、完整 workspace 仍未验收。

首版原探针复测：`cargo test -p deepagent-app-core --test kernel_v2_e2e active_rewind --offline -j 1 -- --nocapture`，**2 passed / 0 failed / 0 ignored**，0.33s；忙时明确拒绝且历史未动，实际 invocation 结束后回退成功，两种回答投影均被移除，原 run 单终态保留。持久化同一专项修正后 **7 passed / 0 failed / 0 ignored**，5.09s。并发编译期间一次 cargo fmt 遇 Windows 源码映射 error 1224，未完成，待 rustc 退出后单独重跑；不把该次随后的测试成功算作格式成功。新增直接 RuntimeEngine/Kernel、接纳与检查点回归尚待验。

调用方补强：桌面生产回调在后端返回之前删除 liveTranscripts 并清屏，busy 错误只提示、不恢复原屏幕。先执行 `node --test --test-name-pattern='rewind refusal' scripts/check-session-control.test.cjs`（apps/desktop）得到 **0 passed / 1 failed**，实际 messages=[] 而非原对象。只将失效缓存/清屏移到 await rewindSession 成功之后；同一文件 AST 提取、转译、实际执行回调，拒绝/请求中不变、成功刷新、等待时切换会话、空会话四项 **4 passed / 0 failed / 0 skipped**。该配套修复归属 H-75，不另计问题。已经接入前端 build 回归门，尚待完整 pnpm build；没有改 Tauri/SDK DTO、组件样式或添加新控件，真实 Desktop 手动交互未验。

四 crate 实跑 `cargo test -p deepagent-persistence -p deepagent-session -p deepagent-runtime -p deepagent-app-core --offline -j 1 -- --quiet` 退出 **0**，编译 5m20s：AppCore lib 1104 passed/71 ignored，checkpoint_audit 18 passed/1 ignored，kernel_v2_e2e 103 passed/91 ignored，persistence lib 67 passed，session_control 8 passed（含子进程 helper 测试，另有 16 次真实线程抢占），runtime lib 284 passed/16 ignored，e2e_stabilization 5 passed，session lib 14 passed，四 crate doc-tests 均 0；各组 0 failed。feature 合并范围不同，不把与 workspace 的测试数量差异当作删测试。本命令编译的 E2E 是追加 provider failure/cancel 两项之前的版本，因此新两项正在专项重跑；不冒称该命令已覆盖后加测试。源码格式随后单独 cargo fmt 已退出 0，前端完整 build 与 workspace/lint 尚待。

最终追加故障项首次专项 `cargo test -p deepagent-app-core --test kernel_v2_e2e active_rewind --offline -j 1 -- --nocapture` 为 **3 passed / 1 failed / 0 ignored**：正常/回退/供应商 400 失败正常，取消用例在未释放离线 transport 时等待 5s 超时。核对 HttpTransport::stream_cancelled，兼容默认仅在 stream 之前检查取消；本屏障夹具此前没有实现实时 flag 处理，不能据此宣称真实 HTTP 取消有缺陷。已在同一离线 transport 实现取消感知 seam（不改生产取消机制），待重跑；保留该夹具失败证据，不通过释放成功响应冒充取消。前端 `pnpm build` 已退出 **0**（包括新增回调 4 项、tsc 与 Vite），Vite 46.70s，有第三方浏览器 externalization/大 chunk 警告，未声称零告警或实际 Tauri 点击验收。

取消感知夹具修正后，最终 active_rewind 专项 **4 passed / 0 failed / 0 ignored**（0.33s）：正常、不取消的 busy 拒绝、供应商 400 failed、实时 flag 取消四条正式 ChatService 链均通过；后两组无回答落盘，原 RunStore 为 failed/cancelled 且 run_events 单终态，实际调用退出后回退成功。已开始 `cargo test --workspace --offline -j 1 -- --quiet`，仍不将本项先标验收完成。

最终 workspace 实录：上述命令编译 **6m52s** 后运行，退出 **1**。AppCore lib **1104 passed/80 ignored**、checkpoint_audit **18 passed/1 ignored**、kernel_v2_e2e **105 passed/91 ignored**（已含最终四种 active_rewind 场景）、builtins **292 passed/1 ignored**、persistence lib **67 passed**、session_control **8 passed**，这些组均 0 failed；之后 prompts lib **27 passed/1 failed**，既有 H-306 的 `creative.video-edit.v1.defaultUserPrompt` 为空。Cargo 早停，后续 target 不计本次 workspace 通过；前述四 crate 退出 0 已独立验证 runtime/session，但不能替代全 workspace 结果。

最终 lint 实录：`cargo clippy -p deepagent-persistence -p deepagent-session -p deepagent-runtime -p deepagent-app-core --all-targets --offline --no-deps -j 1 -- -D warnings` 退出 **1**，被未修改的 `run_store.rs:127 explicit_counter_loop` 阻断。随后四 crate `--lib` 普通 lint（不带 `-- -D warnings`）退出 **0**，仍有既有 run_store/model_agent/context_runtime/plugin_preparation/run_assembler warnings；`cargo clippy -p deepagent-persistence --test session_control --offline --no-deps -j 1` 退出 **0**，仅带同一生产库 run_store 告警，没有本项测试文件告警。没有叠加豁免、顺手修基线或冒称严格/all-targets/全依赖 lint 通过。

补充目标 lint：`cargo clippy -p deepagent-runtime --tests --offline --no-deps -j 1` 退出 **0**，带既有 model_agent/schedule_audit warnings；`cargo clippy -p deepagent-app-core --test checkpoint_audit --test kernel_v2_e2e --offline --no-deps -j 1` 退出 **1**，被未修改的 `mcp_discovery_restore.rs:433 panicking_unwrap` 阻断，另有既有 responses_custom_tool 布尔断言 warning，不把两个 target 记通过。最终 `cargo fmt --all -- --check`、`git diff --check` 退出 **0**；暂存后仍须检查新文件与台账。

边界复核：共享活动 scope 只协调同一 session 的运行与 rewind/会恢复工作区的 AppService fork，不替代 InputLeaseRegistry，不解决 H-33 多宿主之间的运行互斥。取消只是设置状态，所有权必须由真实执行退出后释放；等待中的 admission 仍算忙。直接嵌入方若跨调用持有历史快照，应在恢复快照前获取 EventStore activity 并持有到最后写入；本项不使任意旧 Session 自动刷新，也没有给离线/外部直接写日志的代码补分支身份。不同 session 共享 workspace、后台笔记/知识派生状态（H-23）、脱离 Future 生命周期的本地进程或 native 副作用，不在本次动态证明范围。锁是 fs2 协作式并发边界，不是沙箱；旧版本宿主不识别新控制锁，因此不能混跑新旧二进制获得本项保证。不改事件格式、DB schema、供应商字段或 DTO，新增空 control.lock 文件沿用稳定锁目录保留。

证据链：E-H75-01 为原 HEAD `1a3c7192` active_rewind 命令与迟到回答失败观察；E-H75-02 为最终同一专项 4 passed/0 ignored、四 crate 回归及持久化控制 8 项；E-H75-03 为桌面回调原拒绝场景 0 passed/1 failed、修正后 4 passed/0 skipped 和 pnpm build 退出 0；E-H75-04 为上述全量及 lint 实际退出和基线阻断。source_type=command，observed_at=2026-10-26，content_hash/artifact_path=n/a，未额外保存敏感模型日志。F-H75（severity=P1，category=design，status=validated，confidence=high，location=RunCoordinator::admit / AppService::rewind_session / EventStore 控制 scope）由 E-H75-01 证明缺陷，E-H75-02/03 证明上述本机行为，E-H75-04 保留全量未通过边界。P-H75（path_type=callflow）：接纳取得共享 scope → 原调度/快照恢复 → 原 Kernel/工具循环 → 外层失败/终态投影 → 释放活动保护 → 独占检查点恢复与 truncate；每步关联 E-H75-02，桌面成功后刷新关联 E-H75-03，原失败路径关联 E-H75-01。范围/授权见文首，时间线为本节按执行顺序追加记录。code-audit 影响是人工复核完整写入生命周期；docs-generator 影响是将原失败、夹具错误、本机验收与未验范围分离留档，未用扫描结果代替复现。

交付前校验：Rows=306、Unique=306、Missing=[]、BadColumns=0；已修复 4 / 历史跳过 17 / 归并 7 / 撤销 2 / 待修复 276 / 修复中 0。显式暂存本项 15 个文件（含两个新增测试文件），`git diff --cached --check` 通过；用户原有五类未跟踪项不读、不改、不暂存。尚未创建提交，此刻不猜填 hash；提交后另做文档回填，不 amend、不推送。下一项 H-76 只排队，未开始复现或修改；整体 goal 保持 active。

提交后回填：本项独立修复提交为 `8eea7bd5`，恰含上述 15 个文件；此段与表头、H-75 行、原审计状态同步更新，保留前段提交前的真实时间线。未推送、未创建 PR，未改动用户原有未跟踪项。剩余有效待修复 276；总工时仍为 200–400 有效小时初估，环境等待另计，没有足以机械校准吞吐的有效工时记录。

## H-76 本轮实施与验收明细（2026-10-26）

当前状态：**已修复并验收，限所列 Windows 本机行为**。三种 mutation 共用当前字节版本/已读窗口/暂存后复核/发布边界，拒绝陈旧覆盖而不替代权限/检查点。代码、测试和当时两份文档已独立提交为 `b7cdfcc8`（9 文件）；本次仅回填文档，不 amend、不推送。以下保留阶段性失败和当时尚待项，不覆盖本节最终限制。

交付前核对：`cargo fmt --all -- --check`、`git diff --check`、显式暂存 9 个本项文件后的 `git diff --cached --check` 均退出 0；台账 306 行、306 唯一编号、无缺号、8 列正确，已修复 5/历史跳过 17/归并 7/撤销 2/待修复 275/修复中 0。原有五类未跟踪项未读、未改、未暂存。所有本轮测试进程已结束；下一项 H-20 仅排队，尚未开始其原探针复核或生产修改。

最终补跑 `cargo test -p deepagent-runtime -p deepagent-session --offline -j 1 -- --quiet` **退出 0**：runtime lib **284 passed/16 ignored**、e2e_stabilization **5 passed**（54.68s）、tokenizer 正常项 **1 passed/1 ignored**、schedule **2 ignored**、session **14 passed**，两 crate doc-tests 均 0。未执行项不作通过；完整 workspace/严格 lint 的基线失败仍保留，不影响原 H-76 本机不变量已验的事实，也不等于整个任务完成。

以下先列最近一次专项/契约校验进展，随后保留修复前与首版实录；“尚待”均指该条记录当时状态，最终验收另列。

修正契约断言后同一 AppCore 专项：**3 passed / 0 failed / 0 ignored**（1.74s，编译 1m03s）。三种 mutation 的陈旧拒绝、外部全文保留、重读成功，下一 wire 与重开 SQLite 的唯一原生基础 envelope 完整相等、action failed/completed、单 succeeded run 终态均通过。不要求一次工具冲突使整个 run 失败，不证明真实供应商行为；workspace/lint 收尾仍待。

严格 lint 实录：`cargo clippy -p deepagent-persistence -p deepagent-builtins -p deepagent-app-core --all-targets --offline --no-deps -j 1 -- -D warnings` 退出 1，被未修改的 `persistence/src/run_store.rs:127 explicit_counter_loop` 阻断。不增加豁免、不顺手修改，不能列为通过；workspace 仍在执行。

查证首跑断言差异：Session `response_items_for_payload` 持久化基础 status/error，ModelAgent observation 与既有 `failed_observation_is_fed_back_with_recovery_hint` 测试明确为热请求追加 tool/recovery_hint。因此测试改为成功完整相等、失败完整基础 envelope 相等且额外核对 tool/非空 hint，不以 substring 代替、不改生产投影；待重跑。这不等于已验所有热/冷历史投影差异。

最终严格相等 E2E 首跑：`cargo test -p deepagent-app-core --test kernel_v2_e2e file_version --offline -j 1 -- --nocapture`，**1 passed / 2 failed / 0 ignored**（0.86s，编译 1m54s）。持久化保留 status/error，下一 wire 多了 tool/recovery_hint，导致完整 JSON 相等断言失败；正常 write 两组通过。先核对既有投影契约，不以修改生产实现来迁就测试，仍未最终验收。

修复前在 HEAD `4c885044` 执行原命令 `cargo test -p deepagent-app-core --test kernel_v2_e2e file_version --offline -j 1 -- --include-ignored --nocapture`，**1 passed / 1 failed / 0 ignored**，0.88s。陈旧读取的 write_ok=true、外部 owner 被覆盖；未修改和重新读取对照均成功。授权/网络范围同文首，所有样本为临时目录虚构内容，不新增发现或操作用户数据。

首版根因方案（本地设计，尚未最终验收）：三种 mutation 共用一个观察版本/暂存/最终比较/发布边界，读工具按真实字节而非 mtime/长度决定是否返回空桩。缓存区分已加载字节和已交付窗口，整文件替换要求完整覆盖，局部编辑仅改已读范围；多次编辑追踪内存替换后的范围。原子替换仍复用旧平台原语，新路径使用既有 no-replace 发布。借鉴 Claude FileWriteTool 的实际写入前版本复核及无 await 区段，不复制业务代码，也不声称参考实现解决任意外部并发。

复用 persistence 的 fs2/sha2 实现稳定路径协作锁，四种 read/write/edit/multi_edit 从同一临界区进入，缓存与宿主可独立；锁位于相同 OS 临时根的散列命名空间，不删除活动锁文件，覆盖普通父目录别名，不是授权/沙箱或任意文件系统 CAS。检查/暂存/最终重读/发布/缓存失效放在一个 blocking 操作内；非协作写入在最终比较与 rename 的 syscall 间隙仍可竞争，旧二进制/不同临时根不受该锁协调，不能夸成通用多写入者事务。H-238 actor 归属、H-260 压缩后可见性、H-287 DACL、H-78 路径授权及 delete/move/native 后台副作用仍是独立范围。

首版实际 `cargo test -p deepagent-builtins --lib file_ --offline -j 1 -- --nocapture`：**56 passed / 0 failed / 0 ignored**，测试体 0.73s；筛选也命中一个既有 bash 临时文件测试，不把它说成 56 个新场景。出现本项移除旧 helper 后残留的 PathBuf 未使用告警，已移除，待后续重跑；既有预期 panic 已 catch，不是测试失败。此时仅既有 file 筛选回归，尚未新增故障/并发回归或复测正式原探针，不能关闭 H-76。

首版正式原探针同命令复测 **2 passed / 0 failed / 0 ignored**（0.66s，编译 2m05s），外部 owner 保留、陈旧写入返回失败，两个正常对照保持成功。此时二进制尚未包含后来追加的独立 mutation 回归及扩展 E2E；不能冒称已经覆盖 edit/multi_edit、文件重开或多宿主。追加测试使用 `FileTimes::set_modified`/`File::set_times`，已查 [Rust 官方文档](https://doc.rust-lang.org/std/fs/struct.FileTimes.html) 为 1.75 起可用；当前 workspace 声明 Rust 1.80，尚未执行该 MSRV 编译。仍不关闭 H-76。

追加首批实录：`cargo test -p deepagent-builtins --lib file_mutation:: --offline -j 1 -- --nocapture`，**18 passed/0 failed/0 ignored**（0.24s，含一个子进程 helper，并非 18 个独立故障）；同内容改时间、同长度同 mtime 改内容、实际窗口与完整覆盖、Unicode/CRLF 多次编辑、暂存后更改、no-replace 竞争、发布故障/panic 清理、12 次双缓存竞争和独立子进程正式文件工具均通过。预期 panic 由 catch_unwind 捕获。之后同一 E2E 命令移除 include-ignored 为 **3 passed/0 failed/0 ignored**（1.55s），原探针已取消 ignore，write/edit/multi 的陈旧拒绝和重读成功、下一 wire 错误、durable action 和新文件 SQLite 句柄均已核对。

上述二进制尚未包含随后增加的强杀 owner/实际排他锁测试、等待期间取消回归，以及原生结果与 wire 的严格相等断言，不能用它们证明后加项已过。源码进一步缩短 cache mutex 持有范围，磁盘临界区使用路径 gate 而非串行化所有无关文件。复用现有 ToolExecutionContext 在 blocking 开始、取得锁后和最终提交前检查取消，不另建取消 flag；正在运行的 native syscall 不能撤销，Future 被丢弃也不等于停止 blocking 线程。记录的观察窗口是文件工具产出的窗口，不证明后续 Runtime 截断/压缩或其他 actor 的最终可见性，H-238/H-260 不关闭。新增源码和故障项还需重跑。

H-76 交接前最新专项实录（2026-10-26）：`cargo test -p deepagent-builtins --lib file_mutation:: --offline -j 1 -- --nocapture`，**20 passed / 0 failed / 0 ignored**，测试体 5.07s、编译 17.24s；含一个 subprocess helper。独立进程持锁/5 秒竞争超时/强杀回收后重取、入口前和等待 gate 期间取消不发布/正常重试均通过，覆盖最后 context/cache mutex 修改。预期 panic 已捕获，不是失败。AppCore 严格 wire/persisted equality、workspace、lint 尚待，仍为修复中。

H-76 人工复核与兼容边界：四工具 `invoke` 统一进入各自 `invoke_with_context`，三种 mutation 再进入同一 `file_mutation::apply`，无递归调用或第二条写链。路径 gate 先于 cache mutex，cache 锁只取观察快照/失效，不跨整个磁盘暂存；范围按反向替换维护，不扩到未读后缀。已有工具参数、成功结果字段、权限/审批流水线、provider、协议 DTO、数据库 schema 和前端未改；新增 persistence Rust 协作函数，不是新权限中心。大文件整写必须分页读全、外部变化明确冲突，是有意收紧的执行契约。

Workspace 实录：`cargo test --workspace --offline -j 1 -- --quiet` **退出 1**；Cargo Finished 为 7m01s（含构建锁排队，不作有效工时）。AppCore lib **1104 passed/80 ignored**、kernel_v2_e2e **107 passed/90 ignored**、checkpoint_audit **18 passed/1 ignored**、builtins **312 passed/1 ignored**、persistence lib **67 passed**、session_control **8 passed**，以上各组 0 failed。之后 prompts lib **27 passed/1 failed**，仍为既有 H-306 的 `creative.video-edit.v1.defaultUserPrompt` 为空；没有修改该问题，早停后未运行 target 不算通过，另补 runtime/session。

普通生产 lint：`cargo clippy -p deepagent-persistence -p deepagent-app-core --lib --offline --no-deps -j 1` **退出 0**，带既有 run_store explicit_counter_loop 及 AppCore context_runtime clone_on_copy、plugin_preparation type_complexity、run_assembler too_many_arguments 警告；本项修改文件无新增告警，但不替代严格或所有 test target lint，不增加 allow。

选定 E2E lint：`cargo clippy -p deepagent-app-core --test kernel_v2_e2e --offline --no-deps -j 1` **退出 1**，被未修改的 `mcp_discovery_restore.rs:433 panicking_unwrap` 默认 deny 阻断，另有 `responses_custom_tool.rs:58 bool_assert_comparison` 既有 warning；没有报告 file_version 修改告警，但该 target lint 未通过，不加豁免、不顺手修复。

聚焦严格 lint：`cargo clippy -p deepagent-builtins --all-targets --offline --no-deps -j 1 -- -D warnings` **退出 0**，包含本项 builtins 全 target；不是 persistence/AppCore/全依赖/全 workspace 严格通过。普通生产 lib lint 与 workspace 此时仍在执行，未提前记通过；Cargo 等待构建锁的时间不算有效修复工时。

限制仍保留：每次 read/stub 实际读取字节，增加磁盘 I/O，未做性能 benchmark；OS 临时根下零内容 sidecar 为稳定锁身份保留，不能在其他 owner 活动时直接删除，未实现通用回收器。5 秒锁争用会返回 busy，不无限等待。gate 持有期间的取消测试观测未结束 Future 并设置取消，不声称精确定位 blocking 线程已进入哪条 native 指令；超时丢弃 Future 或已进入 syscall 不保证后台工作被撤销。最后比较/rename 间隙、不同临时根/旧二进制/非协作 delete/move/shell、hardlink/网络文件系统、H-78/H-238/H-260/H-287 均未因此关闭。未跑其他 OS、MSRV、断电事务、真实编辑器/DeepSeek/参考工程或 UI；本项无前端变更，不虚构 UI 验收。

## H-20 本轮实施与验收明细（2026-10-26）

**状态：已修复并验收，限 Windows / Git 2.54.0.windows.1 的下述本机行为；修复提交 `ae193ea8`。** 根因修复是收敛工作树产出所有权：不再由 scheduler 无条件强制删除，成功文件通过快照进入依赖节点基线，失败/取消遗留仍保留，既有子记录可重开定位/恢复/显式清理。报告和 UI 使用同一输出定位投影，不修改主目录、不建第二套存储。H-22、H-21 等独立问题未关闭；不宣称完整 DAG 工具执行、全 workspace 或严格 AppCore lint 已通过。以下按实际发生顺序保留中间失败，不由最终结果覆盖历史。

当前 HEAD `10229dd8` 原探针实跑：`cargo test -p deepagent-subagents audit_successful_dag_preserves_files_for_handoff --offline -j 1 -- --ignored --nocapture`，**0 passed / 1 failed / 0 ignored**，0.70s。`all_succeeded=true, producer_retained=false, consumer_saw_file=false, main_has_file=false, main_readme="original"`，仍删除了未提交产出；只用临时Git仓库，不触及本项目工作树，不计新发现。正在核对工作树所有权/产出交付边界，尚未实施生产修复，不提前标通过。

实施中（未验收）：沿用现有 WorktreeProvider 保存节点目录，以独立临时 index 生成 Git 快照和宿主 ref；下游对依赖快照做无工作副本的合并，再创建 detached 工作树，冲突不执行下游、不修改主目录。唯一工作树名同时作为既有子运行 id，路径落原 SubagentRunStore；报告使用既有 summary 投影定位信息，显式 cleanup 才删除工作树及输出 ref。此快照组合协议是本地设计；成熟参考仅证明保留未知/有变化产出原则，不能冒充其现成 DAG 协议。ignored 文件仍在源目录，不承诺 Git 快照传递；H-22 工具根绑定不在本轮关闭。

首版实际结果：`cargo fmt --all` 退出0；`cargo test -p deepagent-subagents --offline -j 1 -- --nocapture` **14 passed / 0 failed / 0 ignored**，doc-tests 0。原 H-20 探针已取消 ignore，并加强为下游读取 tracked/untracked 的真实字节；观测 `producer_retained=true, consumer_saw_file=true, main_has_file=false, main_readme="original"`。只证明首版调度/提供器，尚未覆盖多依赖冲突、恢复定位、显式清理和 AppCore 编译；本项仍修复中。

扩展首次实跑同一命令 **21 passed / 2 failed / 0 ignored**：`snapshot_preserves_source_index_head_and_main_dirty_state`、`diamond_fan_in_combines_outputs_without_reapplying_common_changes` 的 LF 严格断言分别读到 CRLF。根因是本机 Git checkout 换行策略；夹具补 `.gitattributes: * -text` 以固定本测试的字节语义，不修改本项目或全局 Git config。生产语义仍遵循仓库 attributes/filters，不保证对这些转换后的文件逐字节复制。

后续实际结果：subagents 全 crate 先 **23 passed**，加入 Git 锁定工作树的显式清理失败回归后 **24 passed / 0 failed / 0 ignored**，doc-tests 0。首轮 `cargo clippy -p deepagent-subagents --all-targets --offline --no-deps -j 1 -- -D warnings` 被本轮两处测试的 `cloned_ref_to_slice_refs` 阻断，已按诊断改为 `std::slice::from_ref`，等待重验。AppCore 原 `subagent_runner::tests::` 选择器 **7 passed / 7 ignored**，不算新增验收；新增重开测试首次误用短名加 `--exact` 导致 **0 tests / 1177 filtered out**，不计通过，接下来用完整路径重跑。

完整路径实跑 `cargo test -p deepagent-app-core --lib subagent_runner::tests::dag_outputs_reopen_through_existing_child_store_and_cleanup_exact_tree --offline -j 1 -- --exact --nocapture` **1 passed / 0 failed / 0 ignored**，测试体2.21s：生产 DAG executor/子模型循环（离线文字响应）→原子运行记录→文件 SQLite 真正关闭重开→原目录恢复→显式清理只删除目标树与其 ref。没有使用模型文件工具来证明 H-22。上述 subagents 严格 lint 重跑退出0。随后额外补充 shared DAG transcript 的 `workspace {version:1, kind:dag, path}` 标记及“显式清理后恢复拒绝”断言；这部分尚待最终全量重验，不把旧一次通过视为新代码通过。

实现边界：首次工作树创建时冻结主仓库 HEAD，Git 输出快照包含已跟踪文件当前工作副本、删除和非 ignored 新文件；源 index 的暂存版本、源 HEAD 及主工作区不移动。输出 ref 使用 `--no-deref` 且只允许首次创建，已有 ref 明确失败；宿主诊断保留有界合并冲突路径。子执行成功但发布失败是节点交付失败，不篡改已落盘的子执行成功状态。工作树与快照会占用磁盘，需用户经既有 task.cleanup 显式清理；清理会丢弃该目录中未另行交付的 ignored 文件，不能承诺可恢复。自动交付不覆盖 ignored 文件、submodule 内部工作副本、外部路径、仓库过滤器处理之外的字节身份；不自动合入主目录，不新增 Git 推送。未来丢弃/取消的回归只证明保留产出，不证明 OS 子进程树全部退出或断电原子性。恢复 metadata 是现有 transcript 的可选版本化字段，旧记录保持原兼容行为，无数据库 schema 迁移、无新 run/event/tool/审批存储；Rust WorktreeProvider 新方法与 SubAgentResult.output 的本仓库消费方同步更新，未验证仓库外 source consumers。

workspace 首轮实际结果：`cargo test --workspace --offline -j 1 -- --quiet` 编译5m16s；AppCore **1105 passed / 1 failed / 80 ignored**，测试体14.26s，退出1。未修改的 `completion_plan::audit_tests::discovered_real_build_completes_after_release` 等不到临时 `build-started` 标记；后续 target 未执行，不算通过。该命令编译期间还追加了本项引用保护及边界断言，因此不能当作最终源码验收，最终冻结源码后重新运行串行测试。台账检查首次正则只接受全角冒号、漏匹配 `H-305 撤销`；修正后实测306行/306唯一编号/无缺号/每行8列，当前状态5已修复、17历史跳过、7归并、2撤销、1修复中、274待修复。

冻结源码后的 `cargo test --workspace --offline -j 1 -- --quiet --test-threads=1`，编译5m12s，AppCore lib **1106 passed / 0 failed / 80 ignored**（48.22s），包含本轮最终的 SQLite 重开/事件路径重放/目录不可用拒绝/清理后恢复拒绝与报告/UI summary 投影测试。此前窗口失败在此串行运行通过，但不抹去首轮失败或声称已确定其全部调度根因；workspace 此时仍在运行 kernel_v2_e2e，尚未列全量通过。

同一最终 workspace 后续实际结果：kernel_v2_e2e **107 passed / 0 failed / 90 ignored**（78.24s），builtins **312 passed / 0 failed / 1 ignored**（74.37s），CLI **88 passed / 0 failed / 27 ignored**；进程此时仍未结束，不将 ignored 或后续未运行项作通过。

最终测试已结束：上述串行 workspace **退出1**，最终停在未修改的 H-306：`canvas_prompt::tests::every_node_kind_owns_exactly_one_profile`，`creative.video-edit.v1` 的 `defaultUserPrompt` 为空；prompts **27 passed / 1 failed**。persistence lib **67 passed**、session_control **8 passed**；未到达的 runtime 等 target 不算通过。单独执行最终 `cargo test -p deepagent-subagents --offline -j 1 -- --nocapture` **25 passed / 0 failed / 0 ignored**（3.61s）、doc-tests 0，含引用不可覆写/不解引用、清理失败、diamond、多输入冲突、字节交付、源 index/HEAD 保持、并发身份、执行/发布失败及 Future drop。最终 `cargo clippy -p deepagent-subagents --all-targets --offline -j 1 -- -D warnings` **退出0**，含该 crate 依赖检查。`cargo clippy -p deepagent-app-core --all-targets --offline -j 1 -- -D warnings` **退出1**：未修改的 models/client.rs:355 `clone_on_copy` 阻断；不加 allow、不夹带修复。AppCore 自身 no-deps 严格 lint 尚在运行，未提前标通过。

最终 lint 收尾：`cargo clippy -p deepagent-app-core --lib --offline --no-deps -j 1 -- -D warnings` 和对应 `--all-targets` 命令均 **退出1**，在未修改的 context_runtime.rs（8处 clone_on_copy）、plugin_preparation.rs:14（type_complexity）、run_assembler.rs:1687（too_many_arguments）共10项诊断处阻断。`git diff --exit-code HEAD --` 这些文件及 models/client.rs、canvas_prompt.rs 无差异；不改它们、不 suppress lint。`cargo clippy -p deepagent-app-core --lib --offline --no-deps -j 1` **退出0，保留同10项 warning**，不能替代严格通过。最终 `cargo fmt --all -- --check` 与 `git diff --check` 均退出0；换行提示为仓库 CRLF 规则，不是内容检查失败。

未执行：Linux/macOS/MSRV、真实 DeepSeek 或其他供应商、真实 Desktop UI、断电事务、完整 Git submodule/LFS/自定义 filter 场景、性能/磁盘占用 benchmark、全 features lint。主机验证仅 Windows、真实 Git 与离线模型 transport；没有前端或 Tauri DTO 变更，不虚构 UI 构建/截图通过。技能只用于人工调用链复核及 Evidence→Finding→Path 的即时记录，不声称运行 Semgrep/CodeQL、真实参考项目或平台安全扫描。总体剩余仍为200–400有效小时初估，尚无独立工时样本，不从本轮编译/串行等待或对话跨度推算吞吐。工作树显式清理测试只删除一次性 TempDir 内测试树；本项目既有用户文件与未跟踪项未操作。

## H-22 本轮实施与验收明细（2026-10-26）

**最终状态：已修复并验收，限下述 Windows 本机范围；代码、测试及当时两份文档已独立提交 `c0c127e3`，本次仅回填文档，不 amend、不推送。** 以下按发生顺序保留“尚未验收”等当时状态，不覆盖原失败，不代表最终状态仍在实施中。整个系统目标保持 active。

范围：只修复 DAG 分配目录与真实工具绑定的执行根一致性，复用现有 registry builder、权限/审批、checkpoint 和子运行存储，不处理 H-19 的 dispatch 限制或 H-21 的恢复字段。原有未跟踪项不读、不改、不暂存。

修复前 E-H22-before：接回上一轮测试 session，不重复启动原探针。`cargo test -p deepagent-app-core --lib subagent_runner::tests::audit_dag_child_rebinds_file_tools_to_assigned_worktree --offline -j 1 -- --exact --ignored --nocapture` **0 passed / 1 failed**，编译1m29s、测试体0.26s。真实观测 `child_ok=true, wire_names_child=true, parent_output=Some("hidden tool executed"), child_output=None`。F-H22 保持已验证；调用路径为 DAG executor → with_root 改掉 registry 比较根 → 跳过重绑 → write_file 写 parent。

实施中、尚未验收：registry 来源 root 保持不变，scheduler 的 assigned_worktree 仅保存在每个 runner clone 的执行上下文中；持久记录和临时 assignment 共用原 execution_root 路径选择及目录存在校验，继续由 run_active 统一重绑。普通 shared/worktree、模型环境与 checkpoint 均复用原实现；不全局 chdir、不修改共享父 registry。参考 Claude Code 的 AsyncLocalStorage 派生 cwd 原则，仅做静态对照，未运行参考项目。原探针取消 ignore；正在补不同持久化模式、并发、真实 Git 依赖交付及恢复/权限对照，不以原探针一项成功关闭问题。

阶段结果 E-H22-original-after：原探针取消 ignore 后，`cargo test -p deepagent-app-core --lib subagent_runner::tests::audit_dag_child_rebinds_file_tools_to_assigned_worktree --offline -j 1 -- --exact --nocapture` **1 passed / 0 failed / 0 ignored**；观测 `parent_output=None, child_output=Some("hidden tool executed")`。这是生产修复首版结果，尚未包含后来追加的 workspace_tests 模块，不冒充最终版本验收。新增回归覆盖脚本传输使用每个请求历史的真实 function_call_output 计步，不以共享全局步数混淆并行子运行。`cargo fmt --all` 退出0；完整新增回归尚待执行。

新增回归首轮：`cargo test -p deepagent-app-core --lib subagent_runner:: --offline -j 1 -- --nocapture --test-threads=1` **15 passed / 2 failed / 6 ignored**，测试体7.45s。两个失败尚未作为通过：命令夹具选用 Get-Location，但既有 allowlist 只允许 pwd，实际明确拒绝；文件组合夹具在最后的越界绝对路径触发执行失败，尚需分离正常路径与边界拒绝并核对返回上下文。并发持久化/临时目录隔离、真实 Git 生产模型工具交付+SQLite重开恢复、普通 worktree/shared、deferred 工具审批拒绝、session_search 父项目锚定已经各自通过；仍不关闭本项。

夹具修正后的第二轮专项同命令 **17 passed / 1 failed / 6 ignored**，测试体8.56s：正常读/编辑/搜索/绝对写入及 checkpoint 真恢复已通过，越界读拒绝且写前 checkpoint 明确终止、父文件不变也已通过。剩余是命令 cwd 观测：pwd 调用 status=ok，但 stdout 未含目录，现有断言失败；正在核对实际 shell wrapper/输出契约，不放宽断言或修改生产权限。

命令观测诊断：直接以本机 PowerShell 重现既有 `wrap_powershell_command` 的 exit 尾缀，`pwd; if ($null -ne $global:LASTEXITCODE) { exit $global:LASTEXITCODE } elseif ($?) { exit 0 } else { exit 1 }` 退出0但只输出空行；同尾缀的 `echo $PWD.Path` 退出0且输出真实 cwd。前者为 PathInfo 格式化观测失效，不据此认定执行根错误。本项不修改生产 shell wrapper；Windows 夹具改用已经直接验证、仍在既有 echo 白名单中的纯字符串 cwd 观测，Unix 保留 pwd，并继续要求精确包含 assigned child 路径及未列入白名单命令被拒绝。该 shell 输出现象不计入 H-22 修复成果。

冻结源码后的专项同命令 **18 passed / 0 failed / 6 ignored**，测试体8.28s（编译1m27s）。9 项新增 workspace 回归和转默认的原探针全部实际执行；6 项 ignored 属其他独立 H，不计通过。覆盖真实 read/edit/write/grep、相对/绝对路径、parent 越界拒绝、checkpoint 字节恢复与父级证据、并发持久化/临时 DAG、missing-root 模型调用前失败、普通 shared/worktree、deferred 工具发现及父审批 Deny、UI 项目切换后的 session_search 锚定、Git DAG 上游字节交付和 SQLite 关闭重开后的真实工具 resume、真实 shell cwd 与未列入白名单命令拒绝。没有借助伪造 tool result。`cargo fmt --all -- --check` 退出0；workspace 与 lint 接下来执行，尚未声称整套通过。

全依赖严格 lint：`cargo clippy -p deepagent-app-core --all-targets --offline -j 1 -- -D warnings` **退出1**，被未修改的 models/client.rs:355 `clone_on_copy` 阻断；不顺手修独立问题、不添加 allow。`git diff --exit-code HEAD --` 已确认该文件无改动。随后另跑 AppCore all-targets/no-deps 严格 lint（结果待取得），用于确认本轮目标自身诊断，不冒充全依赖通过。workspace 编译5m17s，已开始1195项 AppCore lib 测试，此刻尚无最终全量结果。

本轮全量中间实录：AppCore lib **1116 passed / 0 failed / 79 ignored**（54.99s），包含冻结后的 H-22 全部默认测试；checkpoint_audit **18 passed / 1 ignored**。workspace 仍在 kernel_v2_e2e，未列最终通过。`cargo clippy -p deepagent-app-core --all-targets --offline --no-deps -j 1 -- -D warnings` **退出1**，context_runtime.rs 8处 clone_on_copy、plugin_preparation.rs:14 type_complexity、run_assembler.rs:1688 too_many_arguments 共10项既有诊断；最后一处仅因本轮构造字段增加1行而移位，函数本身无修改。接下来不带 deny-warnings 跑 all-targets/no-deps，保留 warning，不冒充严格通过。

普通 all-targets/no-deps lint 也 **退出1**：lib 保留上述10项 warning，随后被未修改的 `tests/kernel_v2_e2e/mcp_discovery_restore.rs:433` 既有 `panicking_unwrap`（默认 deny）阻断，另有 responses_custom_tool.rs:58 的 bool_assert_comparison warning。不将“没有 -D warnings”误写为通过，也不以此前仅 lib 的结果代替本次 all-targets。后续单独记录普通 lib 检查结果；这不消除严格及集成测试 lint 的限制。当前源码仅5个本项相关文件（含两份文档），上述两处测试无改动。

本轮相关完整回归：`cargo test -p deepagent-runtime -p deepagent-subagents --offline -j 1 -- --quiet --test-threads=1` **退出0**：runtime lib **284 passed / 16 ignored**、e2e_stabilization **5 passed**（104.61s）、tokenizer_pressure **1 passed / 1 ignored**、schedule_audit **0 passed / 2 ignored**、subagents **25 passed / 0 ignored**；两组 doc-tests 0。workspace 的 kernel_v2_e2e **107 passed / 90 ignored**（75.63s），继续执行其他 crate，此刻仍未结束。普通 `cargo clippy -p deepagent-app-core --lib --offline --no-deps -j 1` **退出0，保留10项既有 warning**，不是 all-targets 或严格 lint 通过。

最终 workspace：`cargo test --workspace --offline -j 1 -- --quiet --test-threads=1` **退出1**，不是全量通过。实际通过 AppCore lib **1116/79 ignored**、kernel_v2_e2e **107/90 ignored**、builtins **312/1 ignored**、CLI **88/27 ignored**、persistence **67 passed**、session_control **8 passed** 等；最终 prompts **27 passed / 1 failed**，仍为未修改的 H-306：`creative.video-edit.v1` 的 defaultUserPrompt 为空。workspace 在此停止，后续 target/doc-tests 不算执行；runtime/subagents 另跑的实际通过结果见上。源码冻结后无再改动，fmt/diff 检查退出0。

边界与交付：仅修改 run_assembler 构造、subagent_runner 执行根派生、独立 workspace 默认测试和这两份文档，共5个文件；不改公共 API/DTO、数据库 schema、模型供应商参数、权限/沙箱配置或前端，不需要数据迁移。未执行其他 OS/MSRV、全 features lint、真实 DeepSeek、真实 Desktop UI、Sandboxie/Windows Sandbox、断电/强杀或完整模型规划模块；未运行 pnpm build，因为没有前端或 Tauri 契约变更。原审计失败、夹具失败、PowerShell 观测诊断、全量与 lint 限制全部保留。技能用于静态调用链与 Evidence→Finding→Path 即时记录，没有假称扫描器或参考项目运行结果。总剩余仍为200–400有效小时初估，环境/编译等待另计，不以本轮墙钟时间估算吞吐。下项 H-23 仅排队，尚未复现或修改。

交付前检查：`cargo fmt --all -- --check`、`git diff --check`、显式暂存本项5文件后的 `git diff --cached --check` 均退出0。台账 **306行/306唯一编号/Missing=[]/BadColumns=0**，已修复7、历史跳过17、归并7、撤销2、待修复273、修复中与阻塞均0。全部本轮测试/lint进程已退出；原有5类未跟踪项未读、未改、未暂存。修复提交 `c0c127e3` 只含本项5文件，本次另做 hash 回填提交；整体未完成，H-23 尚未启动。

## H-23 本轮实施与验收明细（2026-10-26）

**最终状态：已修复并验收，限下述Windows本机范围；修复提交 `b3614d8c`，本次只做文档hash回填，不amend、不推送。** 本节按发生顺序保留中间记录，早期“尚未实施/验收”不覆盖最终状态。最终模型wire均为离线transport，不证明真实DeepSeek服务或摘要质量。完整workspace/严格lint既有阻断保留，不因本项关闭而宣称全绿。

范围：回退后会话笔记的历史归属、提取提交与 Message/native 两条压缩入口；复用现有 EventStore/session-file generation 和 SessionMemoryStore，不改 H-11、H-33 或其他独立问题。开始时 tracked 干净，原有5类未跟踪项不读、不改、不暂存。

E-H23-before：`cargo test -p deepagent-app-core --lib service::tests::audit_rewind_rejects_session_note_from_discarded_branch --offline -j 1 -- --exact --ignored --nocapture` **0 passed / 1 failed**（0.32s）。当前历史已经没有 RED，但实际候选 `Some((4,"Discarded decision: use RED"))` 会替换包含 BLUE 的新分支。F-H23/P-H23 仍成立，原审计证据不是本轮虚构重验。尚未修改生产代码；下一步核对笔记生产、持久化、提取并发和所有消费方，不仅在 rewind 入口清一遍状态。

首版阶段记录（仍修复中）：保存接口改为显式提供来源事件快照，旧7处夹具未同步导致首次编译退出1；同步后 `cargo test -p deepagent-app-core --lib session_memory::tests --offline -j 1 -- --nocapture` **31 passed / 0 failed / 0 ignored**，0.63s。这轮仅覆盖原有模块测试；新增分支、SQLite重开、真实工具编辑暂停/成功/失败/abort 后控制锁释放等测试尚未验收。已修正原报告顶部仍停留5已修复/275未关闭的旧摘要，当前仍7已修复/273未关闭。后续 `cargo fmt --all` 曾遇 Windows 文件映射占用 os error1224 退出失败，不能记格式通过；尚待重试和最终检查。

新增专项实际通过：同一 `session_memory::tests` 命令扩展后 **39 passed / 0 failed / 0 ignored**（2.27s）。这次已覆盖来源尾删除/保留、旧快照拒绝、token/tool水位、新提取请求无旧分支文本、legacy和fork副本、SQLite重开，以及真实工具完成后暂停、跨Database控制拒绝、提取成功/失败/abort后解锁与重建。`cargo fmt --all` 重试退出0。完整 AppCore lib（含原正式探针和native/cancelled-native/Message模型wire）仍在执行；尚未关闭本项。

中间全lib实跑 `cargo test -p deepagent-app-core --lib --offline -j 1` **1126 passed / 0 failed / 69 ignored**，9.54s，原正式探针与三个模型wire分支均通过。但随后源码复核确认真实文件编辑在 `spawn_blocking` 内，仅Future生命周期锁不足以约束取消后的写入，未用这次通过直接关闭问题。已将提取改成pass私有临时笔记，逻辑路径经现有PinnedFileTool映射到working文件；完成才保存来源状态并导出最终笔记，旧/未提交文件不参与下一次输入。没有新增执行器或权限中心；此最终版本仍在重验，强杀时临时目录回收与通用脱离Future的副作用不在本项完整保证范围。

最终源码全lib：`cargo test -p deepagent-app-core --lib --offline -j 1 --quiet` **1128 passed / 0 failed / 69 ignored**（9.56s）。新增 `staged_pin_isolates_a_blocking_write_after_its_future_is_aborted` 让真实spawn_blocking写任务在其Future被abort后继续，观察旧working文件变为LATE RED而新published文件保持NEW BLUE（使用注入Tool验证生产PinnedFileTool seam，不冒充真实builtin syscall内部暂停）；`rejected_state_commit_never_publishes_the_private_edit` 以SQLite trigger拒绝正式提取的状态保存，确认既有export未改、状态无覆盖、控制锁释放。原read/edit、增量提取、token门槛与工具配对测试仍默认运行。`cargo fmt --all -- --check`、`git diff --check` 退出0。

严格 `cargo clippy -p deepagent-app-core --lib --offline --no-deps -j 1 -- -D warnings` **退出1**：既有context_runtime八处clone_on_copy、plugin_preparation:14 type_complexity、run_assembler:1688 too_many_arguments。未报告本轮新增生产代码告警，但这不等于所有target严格通过；未加allow或顺手修改既有项。Workspace与all-targets普通lint仍在执行，尚未作为验收通过。

兼容性/本地设计边界（最终验收前说明）：公开Rust `SessionMemoryStore::save` 增加显式 `&[Event]` 参数，仓内全部调用方已同步；外部直接嵌入调用方需更新。不改模型参数/DeepSeek协议、SessionMemoryState公共字段、UI/CLI DTO或生产SQL schema。原documents JSON保留平铺字段并新增私有source对象；旧无source行与旧文件仍可留存排查，但不会被当作已证明覆盖；达到门槛后重新提取，不能承诺不增加模型费用。source尾事件绑定整个输入而非safe cursor，依赖现有事件前缀不可变/回退重建身份的契约，不检测外部直接篡改权威文件。

提取期间用户回退明确busy；工作文件只在宿主选择的独立临时目录，模型只见逻辑笔记路径。SQLite中有效summary是下一次提取/压缩的权威baseline，最终md是导出；保存成功而导出失败/强杀可能暂时不一致，不能宣称文件+SQLite崩溃原子性。导出复用已锁定tempfile的NamedTempFile::persist替换能力（本机3.27.0源码及Windows MoveFileExW实现已核对），没有使用会先删目标的atomic_file fallback，也没有另造底层替换原语。未验证断电、磁盘满、Windows DACL、非协作外部写、其他OS、真实供应商/参考工程/UI、性能benchmark或不同进程的实际builtin syscall中止；成功/失败/abort与跨Database gate是本机动态证据，detached writer为注入工具共享seam。H-33多宿主运行/提取所有权、H-63附属费用失败归集、H-260旧文件读授权等独立项仍未关闭。预筛/保存复核增加权威日志读取与working文件I/O；临时目录强杀回收不保证。

全量阶段实际结果：`cargo test --workspace --offline -j 1 --quiet` **退出1**，AppCore（workspace启用keychain）1128 passed/78 ignored、kernel_v2_e2e 107 passed/90 ignored、checkpoint_audit 18 passed/1 ignored、builtins 312 passed/1 ignored、CLI 88 passed/27 ignored、persistence 67 passed/session_control 8 passed；随后在既有H-306阻断：prompts 27 passed/1 failed，`creative.video-edit.v1` 的defaultUserPrompt为空。该命令未执行到的后续target/doc-tests不记通过。与单crate默认特性的69 ignored差别来自keychain条件编译，不将ignored增加说成新增通过。补跑 `cargo test -p deepagent-runtime -p deepagent-session -p deepagent-persistence --offline -j 1 --quiet` **整体退出0**：runtime 284 passed/16 ignored、e2e_stabilization 5 passed（59.41s）、session 14 passed、persistence 67/session_control 8，其他target/doc-tests按实际0或ignored保留。

原探针最终显式命令 `cargo test -p deepagent-app-core --lib service::tests::audit_rewind_rejects_session_note_from_discarded_branch --offline -j 1 -- --exact --nocapture` **1 passed / 0 failed / 0 ignored**（1.90s），`keep=4, old_covered=8, current_tail=12, replacement=None, drops_new_branch=false`。并未靠默认忽略使原问题“消失”。普通 `cargo clippy -p deepagent-app-core --all-targets --offline --no-deps -j 1` **退出1**，既有kernel_v2_e2e/mcp_discovery_restore.rs:433 panicking_unwrap默认deny阻断，另有原workflow_audit等warning；不得当作全target lint通过，未加豁免。单lib普通lint和最后专项独立选择器此时仍在等待结果，未提前记录。

最后专项独立选择器：`cargo test -p deepagent-app-core --lib session_memory::tests --offline -j 1 --quiet` **41 passed/0 failed/0 ignored**（0.98s）；`cargo test -p deepagent-app-core --lib context_runtime::tests::branch_note_is_shared_by_all_compactors_and_stale_note_falls_back_on_wire --offline -j 1 -- --exact --nocapture` **1 passed/0 failed/0 ignored**（0.15s）。普通 `cargo clippy -p deepagent-app-core --lib --offline --no-deps -j 1` **退出0**、保留既有10 warning；不把它替代严格/所有target lint。无前端/桥接改动，未执行pnpm构建或真实UI；未执行all-features跨平台构建、完整workspace后续被H-306阻断的doc-tests，不以旧基线代替。

证据链：E-H23-before见原HEAD `5b0c7468` 的失败命令/RED覆盖BLUE；E-H23-after为本节最终41/1/1与AppCore1128结果（source_type=command，observed_at=2026-10-26，source_ref=session_memory/context_runtime/service测试，content_hash/artifact_path=n/a，未保留用户内容或真实供应商日志）。F-H23（severity=P1、confidence=high、status=validated、location=SessionMemoryStore/Extractor与compaction入口）以两条证据区分缺陷和本机修复。P-H23（path_type=callflow）：来源事件快照 → session/尾EventId校验与gate → 私有working/既有PinnedFileTool映射及RuntimeEngine → 显式快照保存 → 可读文件导出；压缩和调度同源校验。授权/范围见本文首段，timeline为本节连续追加记录。code-audit影响是查到已通过首版测试仍未覆盖的spawn_blocking脱离Future写入，docs-generator影响是保留真实失败/中间通过不足/最终验收的区别；没有假称运行Semgrep/CodeQL或成熟参考工程。

当前验收闭环本阶段新增1项、累计8已修复/17历史跳过/7归并/2撤销/272待修复，整体goal仍active。下一项H-33先核对当前实现、原探针和H-32边界；总体仍200–400有效小时初估，构建锁/编译/设备等待另计，缺少独立有效工时样本不机械扣减。仅暂存本轮五个Rust文件与两份文档，不触碰原有未跟踪项；修复commit取得后单独回填，不amend、不推送。

## 按 H 编号的一一对应台账

“待修复”仅表示未关闭，既有条件性风险和未验范围仍按原项保留；不是预先认定本轮能复现，也不是授权跳过先复核步骤。历史/归并/撤销行的修复摘要和验收字段说明其不适用原因，不冒充新修复。

<!-- repair-ledger:start -->
| H 编号、原标题与原报告链接 | 优先级 | 当前状态 | 归并目标 / 跳过依据 | 根因与修复摘要 | 测试命令、结果 / 未执行验收 | 修复 commit ID | 残余、下一步与剩余工时 |
|---|---|---|---|---|---|---|---|
| [H-01：stdio 事件队列满时丢弃关键通知，连接仍表现正常](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-02：Harness ACK 与运行接纳分离，立即中断可能被虚假确认](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-03：Harness 的逐次沙箱 backend 选择未进入实际执行路由](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-04：Harness 恢复与 steer 替换运行会清空显式模型和权限配置](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-05：Chat Completions 往返投影拆散同一次 assistant 工具响应](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-06：MCP 断线恢复自动重发结果未知的工具调用，可能重复产生副作用](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-07：MCP 工具发现忽略分页，只发布首页工具且不报不完整](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-08：MCP 初始化缺少 initialized 通知，严格服务端无法进入就绪状态](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-09：stdio MCP 并发回包按读取者筛选，其他请求的响应会被永久丢弃](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-10：运行结束后预取压缩任务仍继续执行，结果及 usage 无处归集](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-11：以消息内容反查压缩边界，重复回答会让恢复重新带回已摘要的历史](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-12：Streamable HTTP 未保存初始化分配的 session header，有状态远程服务后续请求失败](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-13：AppCore 插件就绪快照与共享技能注册表不同步，新请求可继续声明已禁用技能](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-14：无效压缩保留原上下文时，已完成摘要请求的 usage 一同被丢弃](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-15：审批等待被取消后未清理，运行终态后仍接受迟到回答并授予跨调用权限](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-16：审批直接以 provider call_id 为全库身份，不同运行之间产生幂等冲突](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-17：桌面收到取消后的保存失败，却因对话已 settled 而静默丢弃错误](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-18：输出上限重试后仍截断时，未完成工具调用被补参数并提交](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-19：子 Agent 的工具限制只过滤 schema，实际执行仍可调用被排除的工具](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-20：DAG 成功节点的工作树被强制删除，文件未交付给主目录或下游](system-harness-audit-2026-10-21.md) | P1 | 已修复并验收 | 原探针在10229dd8复现；保留原始失败 | 保留节点树；独立index快照与依赖合并；既有子记录/事件定位、恢复/清理 | subagents 25 passed/0 ignored；AppCore lib1106 passed/80 ignored；kernel107 passed/90 ignored；fmt通过；workspace及AppCore严格lint既有阻断详见明细 | ae193ea8 | 仅本机行为；H-22独立；Git属性/ignored/submodule/磁盘保留边界见明细；剩余274项计入总区间 |
| [H-21：子 Agent 恢复遗漏原始输出契约，schema、phase、label 全部丢失](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-22：DAG 子运行只改变声明的工作目录，文件工具仍绑定父工作区](system-harness-audit-2026-10-21.md) | P1 | 已修复并验收 | 2026-10-26 原探针修复前复现、转默认后通过；非历史跳过 | 保持 registry 来源根不变，assigned_worktree 与持久记录统一派生有效根，复用原重绑/权限/checkpoint | 专项18 passed/6 ignored；AppCore1116/79 ignored；真实Git交付/重开恢复通过；workspace H-306失败及lint既有阻断见明细 | `c0c127e3` | 限Windows/离线模型/Direct；H-19/H-21/H-78等独立保留；总体200–400有效小时初估 |
| [H-23：回退会话后旧笔记仍按复用序号宣称覆盖，压缩候选会替换新分支](system-harness-audit-2026-10-21.md) | P1 | 已修复并验收 | 2026-10-26原失败保留，限Windows本机验收 | 来源尾EventId/session、显式快照与共用有效性、activity gate、私有working隔离旧后台写 | 模块41/原探针1/native-wire1通过；AppCore1128；完整workspace与lint既有阻断见明细 | b3614d8c | 外部save调用需传快照、旧笔记重提取、busy/导出/崩溃与I/O限制见明细；总体200–400有效小时初估 |
| [H-24：较长的非 ASCII 供应商错误触发 panic，结构化错误处理被中断（已修复）](system-harness-audit-2026-10-21.md) | P2 | 历史已修复跳过 | 原报告明确记录修复实现与修复后验收 | 保留现有实现，不重复修复 | 历史命令与结果见原项；本轮未重跑，不声称重新验收 | 历史提交未在本轮重新定位，不猜填 hash | 保留原项未验限制；本阶段不另估修复工时 |
| [H-25：丢弃供应商 Retry-After，在服务恢复前耗尽限流重试（已修复）](system-harness-audit-2026-10-21.md) | P2 | 历史已修复跳过 | 原报告明确记录修复实现与修复后验收 | 保留现有实现，不重复修复 | 历史命令与结果见原项；本轮未重跑，不声称重新验收 | 历史提交未在本轮重新定位，不猜填 hash | 保留原项未验限制；本阶段不另估修复工时 |
| [H-26：标准 PreToolUse 输出中的 deny/ask 被忽略，实际工具照常执行](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-27：项目 hooks 信任开关只覆盖独立文件，settings.json 内联 hook 仍执行宿主命令](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-28：未信任项目的权限配置覆盖用户只读设置，实际写文件没有请求审批](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-29：HTTP 错误正文挂起时取消检查缺失，停止等待退化到正文完成或总超时（已修复）](system-harness-audit-2026-10-21.md) | P2 | 历史已修复跳过 | 原报告明确记录修复实现与修复后验收 | 保留现有实现，不重复修复 | 历史命令与结果见原项；本轮未重跑，不声称重新验收 | 历史提交未在本轮重新定位，不猜填 hash | 保留原项未验限制；本阶段不另估修复工时 |
| [H-30：SSE 实际按 data 行解析，合法多行事件失败，未闭合终态帧却被接受](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-31：同步原生压缩的摘要请求没有运行取消信号，停止必须等摘要返回（已修复）](system-harness-audit-2026-10-21.md) | P1 | 历史已修复跳过 | 原报告明确记录修复实现与修复后验收 | 保留现有实现，不重复修复 | 历史命令与结果见原项；本轮未重跑，不声称重新验收 | 历史提交未在本轮重新定位，不猜填 hash | 保留原项未验限制；本阶段不另估修复工时 |
| [H-32：备用宿主打开数据库先执行无所有权恢复，误终结主宿主的工具动作和子 Agent](system-harness-audit-2026-10-21.md) | P1 | 已修复并验收 | 2026-10-26，限所列Windows本机范围；E-H32-20261026-01～04 | 打开与恢复分离，既有activity/dispatch scope授权按会话恢复，锁内重读，含终态父run孤儿控制；租约仅到期撤销 | 原探针0/1；最终host_lock2/0、owned_recovery7/0、AppCore1131/78、persistence67/session_control9及runtime/session补跑通过；workspace H-306/严格lint既有失败保留，命令见明细 | `c015bc8f` | 旧无owner Rust API收窄；协作宿主须升级，忙session需后续再次恢复，非协作写者/外部副作用停止不保证；其他平台/UI未验，H-44不归并 |
| [H-33：输入租约仅限 ChatService 内存，两宿主可同时推进同一会话](system-harness-audit-2026-10-21.md) | P1 | 已修复并验收 | 2026-10-26，限所列Windows本机范围；H-32/H-44仍独立待修 | EventStore独占dispatch在admit前取得，outer registration持有；同coordinator队列共享；input lease登记即归outer，等待项drop清理 | 原探针0/1；最终workspace AppCore1131/78、host_session5/0、E2E107/90、persistence67/session_control9通过；runtime/session补跑通过。完整workspace因H-306失败，严格/all-target lint既有失败；命令与限制见本节和原项E-01～04 | `8ed5d82e` | 协作宿主须全部升级；无跨宿主取消转发、崩溃终态/外部副作用静止承诺，其他平台/UI/供应商未验；余量计入总工时区间 |
| [H-34：并行工具绕过统一执行流水线，单工具超时保护在批量调用时失效](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-35：并行调度跨越串行写屏障，新增无关读操作会改变已有调用的执行语义](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-36：完整工具结果在 turn 结束时被删除，持久化历史与 artifact 索引留下失效引用](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-37：定时任务丢弃审批通知却仍等待人工回答，无人值守运行停在不可达审批上](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-38：正式工作流工具注册表为空，工具失败被包装成节点完成并令整次运行成功](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-39：条件分支只生成标签，未选中的互斥路径仍被实际派发](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-40：专业工作流配置在前端序列化被删除或被后端按另一套字段读取](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-41：其他会话完成会清掉当前画布运行监听和停止入口](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-42：定时调度没有跨进程所有权，同一触发时段可重复派发一次性和循环任务](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-43：运行启动先于事件订阅就绪，提前终态丢失后前端持续等待](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-44：启动恢复绕过统一终态边界，运行表已失败但会话重放仍没有终态](system-harness-audit-2026-10-21.md) | P2 | 已修复并验收 | 2026-10-26，原失败保留，E-H44-20261026-01～10 | 统一规范终态事务，run/task身份文件投影，既有ledger确认与重开幂等补偿；正常kernel/setup/workflow接回同一边界 | 原探针默认通过；recovery14/1、golden3/0、AppCore1132/78、core41、persistence67/control9、runtime284/16及session14通过；workspace H-306/严格lint失败保留，见明细 | `3d247d1a` | 无schema迁移；外部last()消费者与Rust构造方需适配；跨平台/断电/真实UI未验，H-95不归并；总区间200–400有效小时初估 |
| [H-45：父 Kernel 总时限直接丢弃执行 Future，前台子 Agent 没有取消和持久化收尾](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-46：Windows MCP transport 只终止直接进程，非 libuv 服务的普通后代仍继续运行](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-47：新增无关 MCP 配置会取消未变服务的初始化，并将宿主取消缓存为该服务失败](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-48：stdio MCP 探活耗尽后只停止探针，旧能力仍可发现，后续每次调用重新启动完整恢复预算](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-49：备用模型切换只更新模型和预算，实际请求仍携带主模型的输出上限](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-50：移动输入未校验界面快照，已过期和未知引用仍下发设备命令](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-51：项目激活改写共享知识服务，既有运行的记忆读写会被重定向到另一个项目](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-52：知识库锁仅保护最终文件写入，独立宿主的旧索引可把不同笔记静默覆盖](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-53：原生压缩移除相关记忆后，后台检索去重与预算仍保留旧记录](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-54：有效模型参数被桌面深度覆盖，显式思考强度与输出上限没有进入真实请求](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-55：简单思考模式直接携带 Responses 的 none 进入 Chat 适配器，本地校验拒绝整个运行](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-56：宿主就绪快照合并未检查工具重名，已连接 MCP 可让普通聊天在请求前整体失败](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-57：MCP 说明与技能目录仅按已发送身份去重，正文未进入可回放模型历史，后续无状态请求丢失内容](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-58：附件清理把字符替换当作身份验证，空 ID/点目录可删除整桶，父目录 session 可删除附件 root 外数据](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-59：WASM 调用完成后遗留的 watchdog 中断共享引擎上的后续调用](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-60：命令输出管道的等待超时仅丢弃句柄，读取任务仍保留缓冲并继续收集迟到输出](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-61：对抗复核脱离主运行取消，取消后仍可落成功终态，总时限也不清理复核请求](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-62：停滞分类等待没有运行取消，停止要等分类结束，但最终取消映射正确](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-63：对抗复核和停滞分类丢弃已返回的 usage，现有会话成本账本漏记附属请求](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-64：自动完成验证没有运行取消与进程归属，总时限终态后真实 build.rs 仍能写文件](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-65：两处自动验证绕过已配置命令后端，显式构建被拒绝后仍可在宿主执行项目脚本并写出工作区](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-66：Flash 开启 DeepSeek 原生搜索后，本地搜索函数被替换为该模型会忽略的原生工具](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-67：当前轮图片未进入实时模型 Surface，原生镜像也丢附件，已有会话的图片在下一轮仍不出现](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-68：历史图片文件丢失时静默退化为纯文字请求，续聊仍成功](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-69：只限制单张图片，缺少多图序列化后的请求体总预算](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-70：导入哈希未绑定模型历史引用，图片替换后同一附件 ID 静默发送新内容](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-71：发现目录丢弃官方输入模态，文字模型仍收到原生图片请求](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-72：检查点备份缺失时，失败的回退或分叉仍删除当前文件](system-harness-audit-2026-10-21.md) | P1 | 已修复并验收 | 原探针修复前失败、修复后默认通过；不重复登记 | 全 checkpoint 准备/发布/历史提交与补偿边界；共用 no-replace 原语 | 详见本节命令表：checkpoint 13 passed，AppCore audit 8 passed/2 ignored，相关库/端到端通过；workspace/H-306 和 lint 基线限制保留 | `d08c776c` | Windows 本机行为验收；断电/其他 OS/UI 未验，补偿失败有恢复路径；不关闭 H-73/H-75/H-285/H-287；本项本地剩余 0，环境另计 |
| [H-73：回退后的旧分支检查点仍可覆盖新分支文件，调用却返回成功](system-harness-audit-2026-10-21.md) | P1 | 已修复并验收 | 原探针修复前失败，修复后取消 ignore 并通过 | manifest 冻结稳定 source EventId，按权威有效历史筛选；保留旧记录/前缀恢复，旧格式身份不明时拒绝 | audit 17 passed/1 unrelated ignored；checkpoint 14 passed；runtime/session、AppCore/持久化相关回归通过；workspace H-306 与 lint 基线未绿，详见本节 | `d40eef9a` | Windows 本机验收；旧格式可能需人工核对，H-75/祖先快照继承/其他 OS/UI 不在关闭范围；本项本地剩余 0，环境另计 |
| [H-74：分支边界截在源消息与原生镜像之间，保留的消息不进入下一请求](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-75：运行中回退没有取消或提交栅栏，已丢弃 turn 的回答仍写入新历史](system-harness-audit-2026-10-21.md) | P1 | 已修复并验收 | 原 HEAD 1a3c7192 为 1 passed/1 failed，原证据保留 | 复用会话锁：admission 至外层投影持共享 scope，rewind/fork 持独占 scope，忙时拒绝；桌面成功后才清屏 | active_rewind 默认 4 passed/0 ignored；四 crate 测试及 pnpm build 退出 0；workspace/严格 lint 既有失败及实际命令见 H-75 明细 | `8eea7bd5` | 限 Windows 后端及回调/构建；真实 UI/其他 OS 未验；H-33/H-23/跨 session 副作用未解决；下一项 H-76，总区间 200–400 有效工时 |
| [H-76：文件写入只验证“曾经读过”，外部已保存的改动仍被旧内容成功覆盖](system-harness-audit-2026-10-21.md) | P1 | 已修复并验收 | 原 HEAD 4c885044 复现失败，修复后原探针已转默认且通过，保留中间断言失败 | 观察真实字节与已交付窗口，三 mutation 统一提交、暂存后复核；稳定路径协作 gate，成功失效 | mutation 20 passed；正式 file_version 3 passed/0 ignored；workspace 内 builtins312、AppCore1104/正式链107、persistence67；runtime/session补跑通过；完整命令与既有失败见本节 | `b7cdfcc8` | Windows 本机范围；非协作 syscall 间隙、actor/compaction/路径/DACL及其他OS仍有限制；下一项 H-20，总区间200–400有效工时 |
| [H-77：grep 未继承敏感文件读取拒绝，普通搜索可将被禁止的内容送入模型请求](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-78：Windows 工作区目录联接绕过词法路径限制，read_file 读取根目录外文件](system-harness-audit-2026-10-21.md) | P1 | 已修复并验收 | 保留全部阶段失败；H-279仅归并；Windows本机验收范围见E-72 | 共享逐跳授权/初始根身份/操作期pin；Windows读/枚举/mutation/delete/move共用边界，主registry有效FsAccess；复用H-76/H-286提交回滚 | Windows builtins349/4 ignored；正式path_boundary8（含mutation16组）/file_version3/move2通过；UNC/SUBST及跨卷显式通过；Linux305、persistence68+9通过；双crate严格lint通过；workspace H-306/AppCore既有lint失败，E-63～72 | 待本次独立提交后回填 | Unix字符串I/O竞态、macOS/断电/任意SMB未验；非全系统自主I/O统一；无协议迁移；本项Windows本机剩余0，环境另计 |
| [H-79：glob/list_dir 返回被直接读取策略拒绝的敏感文件名](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-80：Responses repair 对外部混合工具类型缺少不变量校验（条件性）](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-81：Responses 完成文本 item 被错误补充全局 fallback（含空 assistant）](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-82：Responses 内容过滤被误判为输出到限，触发预算升级和长度续写](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-83：最终钩子报错前已发布完成，stdio 同一 turn 收到相互冲突的两个终态](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-84：审批请求在 gate 决策前投影，造成重复发送或自动放行误报](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-85：模型目录与余额 GET 没有总时限，响应头或响应体停滞时请求可无限 pending（已修复）](system-harness-audit-2026-10-21.md) | P2 | 历史已修复跳过 | 原报告明确记录修复实现与修复后验收 | 保留现有实现，不重复修复 | 历史命令与结果见原项；本轮未重跑，不声称重新验收 | 历史提交未在本轮重新定位，不猜填 hash | 保留原项未验限制；本阶段不另估修复工时 |
| [H-86：模型发现 GET 的错误正文可把反射的 API Key 写入 `/doctor` 会话历史（已修复）](system-harness-audit-2026-10-21.md) | P3 | 历史已修复跳过 | 原报告明确记录修复实现与修复后验收 | 保留现有实现，不重复修复 | 历史命令与结果见原项；本轮未重跑，不声称重新验收 | 历史提交未在本轮重新定位，不猜填 hash | 保留原项未验限制；本阶段不另估修复工时 |
| [H-87：模型刷新用宿主本地旧快照覆盖另一宿主新设置](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-88：旧模型刷新在退出登录或换 Key 后重新写入旧凭据](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-89：Harness 显式选择未知模型时静默回退到默认模型](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-90：Harness SDK 关闭 writer 后的失败请求残留 pending reply](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-91：Harness 并发事件生产可使实时 eventSequence 倒退](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-92：stdio EOF 直接退出运行时，活动 turn 没有有界收尾](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-93：Chat Completions 路由未转换 Responses 命名工具选择的字段形状](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-94：Chat 兼容投影覆盖相邻 reasoning item，只保留最后一段](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-95：启动恢复把同一 session 的最新中断原因套用到所有未完成 run](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-96：定时任务已派发但落账失败，重启后会再次派发且调度循环退出](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-97：调度器停止不取消或等待已派发的定时运行](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-98：DeepSeek Chat Completions 的 JSON 输出模式被拒绝或静默丢弃](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-99：Chat Completions 静默忽略配置中的 top_logprobs 默认值](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-100：Harness SDK initialize 接受缺少 result/error 的 JSON-RPC response](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-101：Harness SDK 忽略畸形 stdout 行并让请求持续 pending](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-102：InstructionsLoaded 外部 hook 缺少兼容输入必需字段](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-103：工具生命周期 hook 缺少调用 ID 与真实工具结果](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-104：Claude 兼容 hook 公共输入缺少 transcript_path 和 cwd](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-105：Stop hook 缺少必需的 stop_hook_active 字段](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-106：StopFailure hook 缺少必需的 error 分类](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-107：续聊的每个 turn 结束都会触发 SessionEnd hook](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-108：SessionStart/SessionEnd payload 缺少必需的事件元数据](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-109：SubagentStop hook 缺少必需的停止状态与 transcript 路径](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-110：PreCompact/PostCompact payload 不符合事件输入 schema](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-111：PreCompact hook 的 stdout 指令没有进入摘要请求](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-112：工作树生命周期 hook 缺少事件专属字段](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-113：FileChanged hook 字段名与变化类型不符合 schema](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-114：后台子 Agent Notification hook 缺少必需的通知类别](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-115：PermissionDenied hook 丢失工具调用关联 ID](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-116：步数上限终止的失败任务仍触发 TaskCompleted hook](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-117：API 错误终止未派发 StopFailure，反而只在完成被拒绝时触发](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-118：Responses 流式失败丢弃供应商错误码，上下文溢出恢复依赖消息措辞（已修复）](system-harness-audit-2026-10-21.md) | P2 | 历史已修复跳过 | 原报告明确记录修复实现与修复后验收 | 保留现有实现，不重复修复 | 历史命令与结果见原项；本轮未重跑，不声称重新验收 | 历史提交未在本轮重新定位，不猜填 hash | 保留原项未验限制；本阶段不另估修复工时 |
| [H-119：视频状态 HTTP 等待不响应运行取消，停止已受理仍需等待供应商返回（已修复）](system-harness-audit-2026-10-21.md) | P1 | 历史已修复跳过 | 原报告明确记录修复实现与修复后验收 | 保留现有实现，不重复修复 | 历史命令与结果见原项；本轮未重跑，不声称重新验收 | 历史提交未在本轮重新定位，不猜填 hash | 保留原项未验限制；本阶段不另估修复工时 |
| [H-120：WorkflowAgent 的节点 publisher 绕过持久化事件通道，实时状态与输出无法从 run_events 重放（已修复）](system-harness-audit-2026-10-21.md) | P1 | 历史已修复跳过 | 原报告明确记录修复实现与修复后验收 | 保留现有实现，不重复修复 | 历史命令与结果见原项；本轮未重跑，不声称重新验收 | 历史提交未在本轮重新定位，不猜填 hash | 保留原项未验限制；本阶段不另估修复工时 |
| [H-121：前端忽略视频进度事件的 job_id，完成前停止后重跑丢失续跑身份（已修复）](system-harness-audit-2026-10-21.md) | P1 | 历史已修复跳过 | 原报告明确记录修复实现与修复后验收 | 保留现有实现，不重复修复 | 历史命令与结果见原项；本轮未重跑，不声称重新验收 | 历史提交未在本轮重新定位，不猜填 hash | 保留原项未验限制；本阶段不另估修复工时 |
| [H-122：视频生成成功后修改提示词仍隐式续跑旧任务，新提示词没有发给供应商却再次成功](system-harness-audit-2026-10-21.md) | P1 | 历史已修复跳过 | 原报告明确记录修复实现与修复后验收 | 保留现有实现，不重复修复 | 历史命令与结果见原项；本轮未重跑，不声称重新验收 | 历史提交未在本轮重新定位，不猜填 hash | 保留原项未验限制；本阶段不另估修复工时 |
| [H-123：本地与 SSH 终端的分片读取推进到缓存末尾，未返回的输出被游标跳过](system-harness-audit-2026-10-21.md) | P2 | 历史已修复跳过 | 原报告明确记录修复实现与修复后验收 | 保留现有实现，不重复修复 | 历史命令与结果见原项；本轮未重跑，不声称重新验收 | 历史提交未在本轮重新定位，不猜填 hash | 保留原项未验限制；本阶段不另估修复工时 |
| [H-124：系统视觉预处理未纳入运行生命周期，点击停止后仍处理附件并提交主模型（已修复）](system-harness-audit-2026-10-21.md) | P1 | 历史已修复跳过 | 原报告明确记录修复实现与修复后验收 | 保留现有实现，不重复修复 | 历史命令与结果见原项；本轮未重跑，不声称重新验收 | 历史提交未在本轮重新定位，不猜填 hash | 保留原项未验限制；本阶段不另估修复工时 |
| [H-125：SSH 客户端不验证远端主机密钥，连接到未信任服务器仍成功](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-126：MCP stdio 与 HTTP transport 在完整读取/解析前没有响应字节上限](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-127：MCP image 工具结果被降格成 Responses 普通 JSON 字符串，模型收不到图像输入](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-128：停止运行后活动 MCP 工具仍等待响应，取消信号没有到达 MCP adapter/transport](system-harness-audit-2026-10-21.md) | P1 | 历史已修复跳过 | 原报告明确记录修复实现与修复后验收 | 保留现有实现，不重复修复 | 历史命令与结果见原项；本轮未重跑，不声称重新验收 | 历史提交未在本轮重新定位，不猜填 hash | 保留原项未验限制；本阶段不另估修复工时 |
| [H-129：设置页的一次性 MCP 测试连接没有启动截止时间（已修复）](system-harness-audit-2026-10-21.md) | P2 | 历史已修复跳过 | 原报告明确记录修复实现与修复后验收 | 保留现有实现，不重复修复 | 历史命令与结果见原项；本轮未重跑，不声称重新验收 | 历史提交未在本轮重新定位，不猜填 hash | 保留原项未验限制；本阶段不另估修复工时 |
| [H-130：Harness `turn/steer` 可从同一个已替代 turn 重复派生多个运行](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-131：固定总时限会截断仍在持续输出的 provider stream](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-132：Chat Completions 的合法内容过滤终态被当作未知 provider 错误，usage 未进入运行账本](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-133：Responses `response.failed` 携带 usage 时未进入运行用量](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-134：被替代的画布 run 迟到节点事件可覆盖当前状态](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-135：stdio MCP 隐式继承宿主凭据，配置环境不是授权边界](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-136：stdio MCP 请求写入受背压阻塞时，停止信号无法结束发送等待](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-137：MCP 工具目录变更通知被丢弃，后续运行继续发布旧工具](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-138：MCP 结果整对象进入模型上下文，客户端元数据没有独立可见性边界](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-139：后置 hook 报错跳过已执行工具的结果提交，重开后成功写入变成结果缺失](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-140：输入 hook 成功输出的上下文被丢弃，主模型只收到原始输入](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-141：周期 todo reminder 在独立用户回合之间被重置，长期会话永不触发](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-142：运行事件持久化失败被吞掉，实时事件与回放事件发生分叉](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-143：终态持久化失败前已经发布实时完成事件](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-144：DeepSeek Chat 合法终态 `aborted` / `insufficient_system_resource` 被当作未知错误](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-145：Thinking 模式下不支持的 Chat `tool_choice` 组合未在请求前拒绝](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-146：DeepSeek Chat 用户隔离字段被序列化为 Responses 的 `user`](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-147：DeepSeek 工具 schema 名称未按 provider 约束校验，非法、超长和重复名称直接越过请求边界](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-148：Responses 工具调用的空 `call_id` 未在请求边界拒绝](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-149：Responses 重复 `call_id` 被静默去重并跨过 provider boundary](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-150：Responses `input` 与 `instructions` 同时为空仍发送请求](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-151：Responses malformed function call 被填充默认身份并继续提交](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-152：官方 `tool_search_call` 在 Responses adapter 中被静默丢弃](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-153：Responses 规范思考档位贯通缺陷（规范值链路已修复；别名见 H-155）](system-harness-audit-2026-10-21.md) | P2 | 历史已修复跳过 | 原项规范值链已有修复/验收；别名缺口仍属 H-155 | 保留现有实现，不重复修复 | 历史命令与结果见原项；本轮未重跑，不声称重新验收 | 历史提交未在本轮重新定位，不猜填 hash | 保留原项未验限制；本阶段不另估修复工时 |
| [H-154：Responses `user` 只检查非空，非法标识符进入真实 provider 请求（已修复）](system-harness-audit-2026-10-21.md) | P2 | 历史已修复跳过 | 原报告明确记录修复实现与修复后验收 | 保留现有实现，不重复修复 | 历史命令与结果见原项；本轮未重跑，不声称重新验收 | 历史提交未在本轮重新定位，不猜填 hash | 保留原项未验限制；本阶段不另估修复工时 |
| [H-155：Responses 设置拒绝官方接受的 reasoning effort 兼容别名](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-156：同名扩展工具可能误入 `apply_patch` custom 序列化分支](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-157：模型目录丢弃 model-level reasoning effort，Responses 请求不校验所选模型支持档位](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-158：Responses named `tool_choice` 接受官方不支持的 `custom` / `web_search` 类型并进入 wire](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-159：MCP 非法 `inputSchema` 在注册与发布阶段未校验，作为 function `parameters` 进入每次模型请求](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-160：Canvas gateway 把非法显式模型引用解释为自动路由，修通专业节点后可静默改用另一供应商](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-161：画布 LLM 节点的温度和输出上限在 provider adapter 前被丢弃](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-162：前端标为可用的四类专业 AI 节点没有可达的正式模型执行链](system-harness-audit-2026-10-21.md) | P0 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-163：工作流模型调用丢失 DeepSeek usage，成本账本、缓存观测和 token budget 与实际请求分裂](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-164：画布文本请求不在运行取消树内，节点完成态可与运行取消终态分裂](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-165：图片供应商可令宿主下载任意私网 URL，并把响应作为成功 artifact 保存](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-166：DeepSeek Responses 思考模式下 top_p 会被忽略或规范化，设置值不等于有效采样值](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-167：画布图片下载无响应字节上限，超预算数据被完整缓冲并持久化](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-168：易变 instructions 位于历史之前，动态变化截断多轮 KV cache 前缀](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-169：桌面接受任意 `image/*`，不支持的 SVG 被原样发送给 DeepSeek Responses](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-170：历史图片每轮重发完整 base64，未接入 DeepSeek Files API 的持久引用](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-171：Responses 图片角色沿用 Chat 规则，合法 `developer` 图丢失且非法 `assistant` 图静默成功](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-172：Responses 数值参数只在设置层校验，公开 ModelClient 仍发送越界值](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-173：Responses 工具输出图片无法保留，公共消息边界静默降为纯文本](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-174：公开 `replace_messages` 绕过 Responses 工具调用配对修复](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-175：工作流 `http-request` 节点绕过统一网络权限与审批边界](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-176：工作流 `knowledgeBaseId` 是死字段，知识检索节点无条件混合 project/global vault](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-177：未知 `target_node_id` 被当作空工作流成功，重跑/恢复目标没有 admission 校验](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-178：专业画布 `code` 节点标记为 enabled，但脚本从未执行且错误被伪造成成功](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-179：专业画布 `document-extractor` 标记为 enabled，但正式运行没有 executor](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-180：专业画布 `human-input` 节点不进入统一人工输入/审批生命周期](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-181：专业画布 `iteration` 不消费前端输入数组，也没有真正调度迭代体](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-182：专业画布 `answer` 节点忽略 `answerTemplate`，成功运行却输出空答案](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-183：专业画布 `variable-aggregator` 忽略 `aggregatorVariables`，成功运行却输出空数组](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-184：工作流 `start` 的必填输入缺少 admission 校验，缺失值被静默填成 null](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-185：工作流 `start` 不校验输入类型，声明 number 却接受任意字符串](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-186：工作流 `start` 没有输入 allowlist，未声明字段进入执行上下文并成功传播](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-187：工作流声明的输入默认值被静默丢弃，缺失字段变成 `null` 仍成功](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-188：工作流字符串 `options` 不参与 admission，非法枚举值成功进入执行上下文](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-189：工作流 `minimum/maximum` 不参与 admission，越界数值成功进入执行上下文](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-190：工作流 `minLength/maxLength` 不参与 admission，越界字符串成功传播](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-191：工作流图没有入口节点仍可成功完成，结构 admission 缺少 start/end 不变量](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-192：工作流图没有 `end` 节点仍以成功终态结束](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-193：start 与 end 彼此不可达时仍按全图拓扑执行并成功](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-194：拓扑等价图仅改变节点数组顺序，执行范围便发生变化](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-195：多个可达 end 没有 reject/join/choice 语义，第一个 end 截断正常分支](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-196：合法 target-node 运行跳过必需祖先，正常节点因缺少中间输出失败](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-197：end 的输出类型声明只校验类型名称，成功结果可违反公开契约](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-198：OR 条件组从 true 开始累加，所有子条件为 false 仍选择 true 分支](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-199：多 case 条件删除 false 端口，但 executor 无匹配时仍返回 false](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-200：Responses 生命周期事件丢弃响应身份、服务端模型与序号，durable run 无法关联完整流](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-201：工作流画布缩放控件改变 zoom 却保留平移偏移，视口焦点随每次缩放跳动](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-202：工作流图持久化不包含视口，重启后用户的画布导航状态无法恢复](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-203：Responses `message` 的公开字符串角色缺少 provider 边界校验](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-204：Harness SDK 对无响应但未断开的 RPC 无期限等待，调用取消后 pending 不回收](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-205：Rust Harness SDK 丢弃通知外层 `eventSequence`，无法可靠做 ACK 或缺口恢复](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-206：Harness SDK 接受协议版本不兼容的 initialize 结果并进入已初始化状态](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-207：Harness SDK 对 `thread/start` 成功结果缺少必填 `threadId` 校验](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-208：Responses `response.completed` 缺少必填 response 字段仍被当作成功](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-209：Responses 输出 `message` item 缺少 `role` 仍被静默补成 assistant](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-210：Responses 流非单调 `sequence_number` 被静默接受](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-211：Harness SDK 将字符串 JSON-RPC response ID 强制折叠为整数，导致请求无法关联](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-212：Harness SDK 未校验 JSON-RPC response 的协议版本](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-213：Harness SDK 忽略 notification method，任意通知可伪装成 Harness 事件](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-214：Harness SDK 请求返回后才订阅会永久丢失先于 response 到达的合法通知](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-215：Harness 归档状态未进入活动列表与运行接纳，已归档 thread 仍可创建 turn](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-216：Harness 冷分叉返回成功但新 thread 未加载，首个 `turn/start` 被拒绝](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-217：未知 namespaced JSON-RPC method 被误报为 Invalid params](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-218：`thread/read` 无界全量物化历史，缺少分页与响应预算](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-219：`tool/list` 是无作用域的静态目录，无法描述当前 run 的实际能力](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-220：`thread/list` 无界返回全部 session，缺少列表分页与服务端过滤](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-221：`config/read` 只返回静态协议 banner，无法读取有效执行配置](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-222：`thread/start` 的 `reasoningEffort` 被静默丢弃，后续 turn 回退到默认思考配置](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-223：刷新相同模型目录会覆盖仍有效的用户选型，下一次 Responses 请求静默换模型](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-224：日/月费用使用错误时间桶，真实预算接纳漏计本期历史费用后继续请求模型](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-225：首次登录只持久化报价，未激活既有 CostService，正常模型有用量却无费用且预算继续放行](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-226：备用模型切换后仍按初始模型结算，完整 usage 被错误归价且下一轮预算误放行或误拒绝](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-227：日/月限额仅驻留内存，宿主服务重建后历史费用保留但预算静默变为无限](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-228：会话搜索在可见性过滤前截断候选，隐藏记录可挤掉全部有效历史](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-229：单会话文件损坏经全库搜索修复阻断无关项目搜索与数据库启动](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-230：单文件验证只按扩展名选择严格 JSON，合法 TypeScript 配置被误报并在 Strict 模式记为失败动作](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-231：可选 ruff 未安装时真实 shell 返回非零结果，被 Python 验证器误报为源代码失败](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-232：回退删除发现事件后，原宿主仍公开旧工具，重建宿主则隐藏](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-233：无 hooks 的公共 Runtime 将并发安全等同于可丢弃预执行，失败响应留下未记录的 Todo 状态](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-234：声明 isolated/read-only 的 agent-hook 共享主运行 TodoStore，正常完成仍替换父任务列表](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-235：模型型生命周期 hook 的成功 usage 未交回会话账本，漏计费用后下一轮预算仍放行](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-236：hook 缺少父运行取消上下文，工具流水线仍等待，MCP 对端未获取消](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-237：agent-hook 未继承主运行有效权限，被规则或工作区边界拒绝的读取仍将正文交给 hook 模型](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-238：agent-hook 与主模型共享“已读”状态，首次读取变空桩且写前凭证串用](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-239：hook 去重忽略条件与模型，不适用的声明可吞掉实际校验器](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-240：一次性 hook 被不匹配的工具提前消耗，首次实际写入没有经过校验](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-241：普通 task 的取消操作遗漏会话归属校验，另一会话可停止不属于自己的后台子 Agent](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-242：子 Agent 的 resume 用状态摘要重启空会话，完整已保存结果及工具观察未恢复](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-243：原生 compaction 移除 memory 摘录后，memory 去重状态未释放，后续检索无法重新注入](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-244：CanvasWorkflowStore 接受陈旧完整图快照，两个窗口的后写会静默丢失先写编辑](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-245：画布偏好读取失败后仍完成 hydration，后续 workspace 设置写回会覆盖未读字段](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-246：画布偏好 hydration 完成后会覆盖读取期间的用户编辑](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-247：画布 legacy 偏好迁移在后端写入失败时先删源数据，导致不可重试丢失](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-248：画布语音合成响应无字节上限，超预算音频被完整缓冲并持久化](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-249：画布语音转写的远程音频输入绕过统一网络权限与审批](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-250：画布 artifact 读取不校验持久化摘要，旧 URI 会静默投影替换后的字节](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-251：命名画布工作流保存了 workspace_id，但库查询、读取和删除没有 workspace 访问边界](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-252：Canvas artifact 的读取、provider 投影、显示和删除按全局 ID 操作，绕过 workspace 隔离](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-253：分块 canvas artifact 上传丢失 workspace 所有权，发布出无主媒体](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-254：artifact 删除先删文件后删索引，数据库失败会留下永久 dangling URI](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-255：命名工作流 ID 可占用 `current:<mode>:<workspace>`，覆盖或删除 live canvas slot](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-256：SDK reader 已确认 EOF，事件订阅仍无期限等待，断线后新订阅也无法得知生产者消失](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-257：SSE 把确定非法的 UTF-8 当成未收齐字符，后续字节持续滞留且错误被误报为缺少终态](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-258：CLI TUI 忽略模型 attempt 回滚，重试成功后仍把废弃答案和推理拼入最终显示](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-259：流式预处理缓存覆盖最终工具参数，实际写入与原生调用历史及续轮上下文发生错配](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-260：上下文压缩清掉模型可见文件结果后，旧 FileStateCache 仍授权覆盖已有文件](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-261：Responses 历史中乱序工具结果被 repair 静默替换为合成失败](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-262：附件身份路径边界（H-58 的重复登记）](system-harness-audit-2026-10-21.md) | — | 归并 | 唯一有效项 H-58 | 只关联原证据，不新增修复任务 | 验收由 H-58 统一记录 | 无独立修复提交 | 不独立计数或估时 |
| [H-263：实时持久化分叉重复登记（已撤销）](system-harness-audit-2026-10-21.md) | — | 撤销 | 原报告 H-265 明确撤销；Git b1912f53 为撤回提交 | 不作为有效缺陷或修复成果 | 不重复运行已撤销探针 | 撤销证据：b1912f53 | 不独立计数或估时；编号不复用 |
| [H-264：MCP 响应无界缓冲（H-126 的重复登记）](system-harness-audit-2026-10-21.md) | — | 归并 | 唯一有效项 H-126 | 只关联原证据，不新增修复任务 | 验收由 H-126 统一记录 | 无独立修复提交 | 不独立计数或估时 |
| [H-265：TypeScript SDK 自动替换已退出进程，却沿用旧握手状态，新进程首条业务请求被拒绝](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-266：TypeScript SDK 的 CLI JSONL 迭代忽略非零退出，部分输出或空输出均无错误结束](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-267：TypeScript SDK 的 CLI JSONL 缺少进程 owner 收尾，停止迭代、解析失败和 close 后仍执行后台工作](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-268：结构化摘要接受空对象与无关 JSON，绕过降级后持续遮蔽原历史](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-269：摘要草稿清理在解析 JSON 前删除字符串内的合法 analysis 标签及正文](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-270：一条计划记录解码失败使全部健康任务不可见，正常创建随后覆盖并删除旧任务](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-271：正常创建的日历不可能计划触发日期搜索 panic，终止整个调度任务且重建后再次触发](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-272：日/月限额只有 run 准入保护，已取得越限用量后正常工具循环仍继续请求模型](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-273：Responses cached_tokens 超过 input_tokens 仍被接受并按低价记账](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-274：Responses token 计数超过宿主 u32 宽度后静默截断，账本可被压成零](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-275：MCP hook 直接调用绕过 discovered inputSchema，非法参数仍发送到工具](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-276：MCP hook 在解析前截断完整拒绝 JSON，超过展示上限后静默放行真实写入](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-277：MCP initialize 结果未校验，缺失必填字段或协商不支持版本仍发布 Ready 并执行工具](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-278：MCP 未知结果重放的真实进程补强（归 H-06）](system-harness-audit-2026-10-21.md) | — | 归并 | 唯一有效项 H-06 | 只关联原证据，不新增修复任务 | 验收由 H-06 统一记录 | 无独立修复提交 | 不独立计数或估时 |
| [H-279：junction 物理路径边界重复登记（归 H-78）](system-harness-audit-2026-10-21.md) | — | 归并 | 唯一有效项 H-78 | 只关联原证据，不新增修复任务 | 验收由 H-78 统一记录 | 无独立修复提交 | 不独立计数或估时 |
| [H-280：敏感路径发现重复登记（归 H-79）](system-harness-audit-2026-10-21.md) | — | 归并 | 唯一有效项 H-79 | 只关联原证据，不新增修复任务 | 验收由 H-79 统一记录 | 无独立修复提交 | 不独立计数或估时 |
| [H-281：中间 system serializer 补强（归 H-168）](system-harness-audit-2026-10-21.md) | — | 归并 | 唯一有效项 H-168 | 只关联原证据，不新增修复任务 | 验收由 H-168 统一记录 | 无独立修复提交 | 不独立计数或估时 |
| [H-282：Git 状态整体 trim 破坏首条未暂存路径，正式模型上下文可把脏工作区报告为 clean](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-283：Read deny 只拦工具，自动工作区上下文仍在首次请求发送 README 与清单摘要](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-284：同一 Harness thread 的第二个 `turn/start` 被确认成独立 started，实际输入租约随后取消第一个运行](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-285：Harness 接受不存在的分叉锚点，超出有符号范围后恢复旧文件却复制完整成功历史](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-286：`move_path` 覆盖目标前先递归删除，移到源的祖先目录时失败却删除源和无关文件](system-harness-audit-2026-10-21.md) | P1 | 已修复并验收 | 原正式探针修复前 1 passed / 1 failed；修复后默认 2 passed | 原生 no-replace 发布；旧目标 staging/失败恢复；失败恢复保留 recovery_path | 详见本文件 H-286 命令表：builtins 292 passed/1 ignored，AppCore 正式链 102 passed/92 ignored；全量/lint 限制保留 | `56b9e7b9` | Windows 本机范围验收；其他 OS、真实跨卷/断电/UI 未验，不等于全局通过；本项本地剩余工时 0，环境验收另计 |
| [H-287：Windows 内容原子替换丢失目标文件 DACL，成功编辑及检查点恢复均未保留原访问控制策略](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-288：计划模式只存在服务内存，恢复同一会话后静默失去写入限制](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-289：CodeGraph 定义文件变更后不会重新连接未修改 caller 的调用边](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-290：画布供应商保存先提交配置、后写密钥，密钥持久化失败后发布新配置与旧凭据的混合身份](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-291：重复编号撤销，归并 H-126（2026-10-25 更正）](system-harness-audit-2026-10-21.md) | — | 归并 | 唯一有效项 H-126 | 只关联原证据，不新增修复任务 | 验收由 H-126 统一记录 | 无独立修复提交 | 不独立计数或估时 |
| [H-292：画布场景声明撤销后旧绑定仍参与实际调用，非瞬时失败阻断健康候选](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-293：共享终端租约过期后真实 PTY 存活但没有续租、接管或清理路径](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-294：完成门把每次成功文件操作强制转换为最终净变化，临时创建后删除被误判未完成并额外请求模型](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-295：TypeScript SDK 以客户端全局审批屏障结束单个 turn，另一会话的待答审批会阻塞已完成流](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-296：模型能力解析以 context_window 是否存在决定整份元数据来源，单独公布的输出上限被 fallback 覆盖](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-297：默认字符密度计数低估官方离线文本，冷计量可漏过既定压缩压力阈值](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-298：Harness 超出存储表示域的读取游标回绕，错误重放全部旧 run events](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-299：桌面端工具循环的实时用量与会话重载显示不一致](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-300：Embedding 工作流执行器存在，但画布契约/图注册表没有注册 embedding 节点](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-301：Audio 合成取消没有打断共享 Canvas HTTP 响应](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-302：画布视频下载没有类型级字节上限](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-303：画布视频 provider URL 第二跳绕过网络审批](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-304：Canvas provider 非 2xx 错误正文在分类前无界缓冲](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-305 撤销：SDK Drop 行为符合已有文档，原探针不能证明独立缺陷](system-harness-audit-2026-10-21.md) | — | 撤销 | f37da4fe 撤销误报；Drop 契约及探针隔离/清理不足 | 不作为有效缺陷或修复成果 | 不重复运行已撤销探针 | 撤销证据：f37da4fe | 不独立计数或估时；编号不复用 |
| [H-306：本地视频编辑的空默认提示词使公开画布提示词目录整体不可读](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
<!-- repair-ledger:end -->
