# 后端可本地化文案（Fluent，zh-CN）。TDD-11 §i18n。
# 仅服务后端产生的通知/错误 key；前端 UI 文案走 react-i18next（apps/desktop/src/i18n）。
# 消息 id 由 i18n key 经 `:`/`.` → `-` 规范化得到（见 i18n.rs message_id）。
# 隐私铁律：禁含转写文本/剪贴板/笔记正文/文件名（AGENTS.md §7）。

notifications-config_recovered = 配置已恢复为安全默认
notifications-permission_revoked = 系统权限已被撤销
notifications-strict_local_blocked = 严格本地模式已拦截联网任务
notifications-prompt_catalog_rejected = 服务端提示词目录不齐备，已改用内置提示词
notifications-prompt_catalog_degraded = 服务端提示词目录按旧清单生效，部分模板改用内置
notifications-fallback = 有一条状态更新需要注意

errors-model_missing = 未找到本地转写模型，请在设置中下载或选择 whisper 模型

# 系统托盘菜单（原生菜单，后端构建，故走后端 i18n）。
tray-show_main = 打开 Lunarca 主窗口
tray-text_quick_note = 文字速记
tray-voice_input = 语音输入
tray-popup_translate = 弹窗翻译
tray-screenshot_translate = 截屏翻译
tray-quick_chat = AI 对话
tray-recording = 开始录音
tray-live_caption = 实时字幕
tray-quit = 退出 Lunarca

# 说话人轨默认标签（导出前缀/会议纪要用；与前端 common:track.* 同语义，PRD-04-053/054）。
track-self = 我
track-remote = 对方
track-remote-n = 对方{ $n }
track-speaker-n = 说话人{ $n }

# 「多条 → 一篇」汇总（PRD-06-047 / TDD-07 §2.1）。错误说明与产物文案都在这里，
# 不在 IPC 里按语言手写两套文字；机器可判的稳定前缀由代码拼在说明之前。
# 条数门槛用参数插入，绝不写死——那会是同一对常量的第三处定义。
merge-stem = 汇总
# 标签改名（ADR-135）：名字不合法时的说明。
tag-rename-invalid = 标签名不可用：不能为空、不能含空格，也不能以 / 开头或结尾。
# 问答摘录里出处已失效的主题页声明（K4）：放在声明正文最前面。
claim-source-changed = 〔来源已变，待核对〕
claim-source-missing = 〔出处已不存在，未核验〕
claim-conflict = 〔本页对此有分歧说法，未判定哪方正确〕

# HUD 窗口标题（系统窗口列表 / 读屏可见；无边框窗自身不画）。
window-title-popup-translate = Lunarca 弹窗翻译
window-title-voice-input = Lunarca 语音输入
window-title-quick-chat = Lunarca 快捷对话
window-title-text-quick-note = Lunarca 速记
window-title-desktop-note = Lunarca 便签
window-title-recording = Lunarca 录音
window-title-teleprompter = Lunarca 提词器
teleprompter-no-script = 提词器里还没有打开脚本。
teleprompter-no-line = 提词器上找不到要标记的那一行，脚本可能刚改过。
teleprompter-retake-once = 重来 · { $times }
teleprompter-retake-many = 重来 ×{ $count } · { $times }
window-title-screenshot = Lunarca 截屏翻译
window-title-desktop-lens = Lunarca 译境
window-title-desktop-lens-trigger = Lunarca 译境控制
merge-tag-stem-suffix = 汇总
merge-sources-heading = 来源
merge-partial-coverage = 注：笔记较多，本次汇总覆盖了前一部分。

# 蒸馏产物落盘时写进正文的标注（D9′ ①）。**必须落盘**：产物是写进 Vault 的普通 Markdown，
# 只在界面上标注的话，重开笔记 / 导出 / 同步到另一台设备之后它与完整笔记一模一样。
# 正文不按归属分叉（三档对**这一篇的读者**是同一件事：这里缺了一截），但也**不许开动作**
# ——「重新整理」对 `caller` 档是一个确定性死循环，下一步该做什么归徽标，它认得归属。
distill-output-truncated-note = 注：本次整理在生成中途达到了单次输出上限，内容不完整；下一步该做什么，见源资料页头的知识笔记徽标。

# 蒸馏起跑线预检：服务端余量装不下这一篇（PRD-13-040 ②）。
# 🔴 三个数**作为参数传进来**，绝不在 Rust 里 `format!` 拼进串：拼一次就等于把这句话
# 钉死在一种语言上，而漏改的那一处会给英文界面的用户一段中文。
distill-quota-short = 服务端余量不够跑完这一篇。整理它约需 { $need } 个输出 token，当前还剩约 { $remaining } 个，差约 { $short } 个。可先补充额度，或在「模型与算力」里改用本机模型。
merge-empty = 没有可汇总的笔记内容。
merge-empty-selection = 请先选择要整理的内容。
merge-selection-too-small = 至少选择 { $min } 条才能整理成一篇；只有一条时请用「生成知识笔记」。
merge-selection-too-large = 一次最多整理 { $max } 条，请分批。
merge-selection-invalid = 所选内容里有一条无法整理，请重新选择。
merge-aggregate-page = 所选内容里有一张主题页，它已经是整理好的一篇，不能当作碎片再合一次。
merge-source-empty = 所选内容里有一条没有正文，请重新选择。
merge-input-too-large = 选中的内容超出一次整理的处理量，请分批整理。
graph-tags-knowledge-map = 知识地图是导航页，不参与关系标签。
review-not-pending = 这一页没有待处理的人工改动。
review-no-baseline = 找不到可恢复的系统底版——请改用「保留我的改动」，当前内容会存为你的笔记。
review-fork-suffix = 我的笔记
ingest-needs-review = 这一页被改动过、尚待复核，先在知识库里处理它的改动。
merge-source-changed = 整理期间来源已变动，本次结果放弃；请重新选择后再试。
merge-target-taken = 整理期间落点已被别的写入占用，本次结果放弃；所选内容没有问题，直接重试即可。
merge-vault-switched = 生成期间已切换笔记库，本次结果放弃。

# 知识维护的两个小调用（体检判定 / 归入要点与锚点）撞上输出上限、正文为空时的说明。
# 🔴 **不说「调大输出预算」**：这三个预算是编译期常量（`checkup.rs` / `ingest.rs`），
# 用户界面上根本没有那个旋钮——把人支去找一个不存在的设置，比只说「失败了」更坏。
checkup-output-capped = 模型把这一次的输出额度用完了却没给出判定（推理模型的思考过程也计在里面）。这一条先跳过，换一个更省话的模型通常就能判出来。
# ── IPC 错误说明（后端产生，经 `ipc::coded_error` 加稳定码前缀）───────────────
# 文案口径：陈述事实 + 可执行的下一步；不使用第二人称口语、不使用感叹号、不解释内部实现。
# 隐私边界同本文件顶注：路径、笔记正文、转写文本一律不进这里（故本节没有任何参数）。

# ── 快捷 AI 对话（PRD-03）────────────────────────────────────────────────────
chat-busy = 上一轮回答尚未生成完毕，请稍候。
chat-empty-session = 当前会话为空，没有可保存的内容。
chat-message-not-found = 该条消息已不在当前会话中。
chat-empty-result = 该条消息没有可保存的内容。
chat-not-chat-note = 该笔记不包含可载入的对话记录。
chat-writeback-no-evidence = 该回答缺少可验证的知识库出处，无法存入知识库。
chat-writeback-unverifiable = 模型补充内容与历史会话不能作为知识库写入依据。

# ── 实时字幕（PRD-04）───────────────────────────────────────────────────────
caption-empty = 当前没有可保存的字幕内容。

# ── 剪贴板（PRD-01）─────────────────────────────────────────────────────────
clipboard-unavailable = 剪贴板正被别的程序占用，请稍后再试。

# ── 语音速记（PRD-02）───────────────────────────────────────────────────────
voice-empty-result = 识别结果为空，无法保存为速记。

# ── 知识地图（PRD-16）───────────────────────────────────────────────────────
knowledge-map-no-topics = 当前没有主题页，无需建立知识地图。
knowledge-map-conflict = 检测到多篇知识地图，请先指定保留其中一篇。
knowledge-map-invalid = 知识地图标识无效。
knowledge-map-review-stale-ownership = 知识地图的维护状态已变更，请重新检查。
knowledge-map-review-stale-body = 知识地图的内容已变更，请重新检查。
knowledge-map-conflict-stale = 知识地图的冲突状态已变更，请重新检查。

# ── 深度体检（PRD-16）───────────────────────────────────────────────────────
checkup-not-ignorable = 该提示会在相关内容处理完成后自动消失，无需手动忽略。
checkup-nothing-to-do = 暂无可核对的主题页声明，且语义索引尚未就绪。
checkup-busy = 深度体检正在进行，请等待当前任务完成。
checkup-parse = 模型输出无法解析为核对结论。
checkup-cancelled = 本次深度体检已取消。
checkup-no-result = AI 引擎未返回结果，请重试。

# ── AI 关系标签与全库重扫（PRD-16）──────────────────────────────────────────
graph-tags-empty = 笔记正文为空，无法提取标签。
graph-tags-busy = 该笔记的关系标签正在生成，请稍后再试。
graph-tags-cancelled = 本次标签生成已取消。
graph-tags-no-result = AI 引擎未返回结果，请重试。
graph-tags-parse = 模型输出无法解析为标签列表。
graph-tags-vault-switched = 生成期间笔记库已切换，本次结果不予保存。
graph-tags-stale = 生成期间笔记已被修改，请重试。
graph-rescan-empty = 没有需要重扫的笔记。
graph-rescan-busy = 标签重扫正在进行，请等待当前任务完成。

# ── 生成知识笔记与批量整理（PRD-06-054 / PRD-13）────────────────────────────
distill-ineligible = 仅录音、转写、对话与记录态笔记可生成知识笔记。
distill-edited = 该知识笔记已被手动编辑，不会自动覆盖。
distill-empty-source = 该资料没有可整理的正文内容。
distill-busy = 该笔记的知识笔记正在生成，请稍后再试。
distill-vault-switched = 生成期间笔记库已切换，本次结果不予保存。
distill-no-preview = 没有可应用的预览结果，请重新生成。
distill-template-limit = 自定义模板数量已达上限。
distill-batch-empty = 所选范围内没有需要整理的资料。
distill-empty = 模型本次未生成任何内容，请重试。
distill-job-payload = AI 引擎返回的结果不是文本，无法用于整理。
distill-job-no-result = AI 引擎未返回结果，请重试。
distill-cancelled = 本次整理已取消。
promote-not-quick-note = 仅语音速记可升级为笔记。
demote-not-note = 只有普通笔记能撤回成速记；资料、主题页与知识地图不行。

# ── 归入主题页与主题页合成（PRD-16）─────────────────────────────────────────
ingest-empty = 模型本次未生成任何内容，请重试。
ingest-cancelled = 本次归入已取消。
ingest-no-result = AI 引擎未返回结果，请重试。
ingest-ineligible-not-markdown = 仅 Markdown 笔记可归入主题页。
ingest-ineligible-material = 资料笔记需先整理为知识笔记，才能归入主题页。
ingest-ineligible-topic = 主题页不能作为归入来源。
ingest-busy = 该笔记的归入计划正在后台生成，请稍后再试。
ingest-vault-switched = 期间笔记库已切换，本次结果不予保存。
ingest-vault-switched-retry = 期间笔记库已切换，请重新发起。
ingest-no-plan = 没有待应用的归入计划。
ingest-suggestion-stale = 该归入建议已失效。
ingest-stale = 归入计划与当前笔记版本不匹配，请重新生成。
ingest-source-changed = 源笔记已改动，请重新生成归入计划。
ingest-nothing-selected = 请至少选择一页后再应用。
ingest-not-topic = 目标不是主题页。
topic-no-sources = 该标签下没有可用于合成主题页的笔记。
topic-draft-budget = 本次主题预览超出处理上限，请减少来源后重试。
topic-draft-incomplete = 模型未完整生成主题预览，本次未保留预览；请减少来源或更换模型。
topic-draft-invalid-output = 模型没有返回可核验的主题预览，未保存任何内容。

# ── 语义索引（PRD-16）───────────────────────────────────────────────────────
embed-assignment-changed = 嵌入模型配置已变更，本次结果不予保存。
embed-shape-chunks-missing = 分片向量不完整，该笔记未写入索引。
embed-vault-switched = 期间笔记库已切换，本次结果不予保存。
embed-profile-mismatch = 嵌入索引标识与模型档位不一致。
embed-no-model = 尚未配置嵌入模型。
embed-nothing-to-do = 所有笔记的语义索引均为最新，无需重建。
embed-busy = 语义索引正在重建，请等待当前任务完成。
embed-cancelled = 本次语义索引重建已取消。
embed-no-result = AI 引擎未返回结果，请重试。
# 🔴 五种契约违规共用这一句：维数、非有限值、零范数、条数、载荷类型对用户是同一件事，
# 差别只对排查有意义（形状事实进日志，稳定码进错误串前缀）。
embed-vector-contract = AI 引擎返回的向量不符合约定，本批内容未写入索引。请确认嵌入模型与当前索引一致，必要时重建索引。

# ── 系统文件对话框（各模块共用）──────────────────────────────────────────────
# 🔴 一条文案，不按调用点分「保存 / 文件夹 / 文件选择器」——对用户是同一件事：他取消了，
# 什么都没有发生。此前这一条有六种说法，正是同一件事被写成六遍的样子。
dialog-cancelled = 未选择任何内容，操作已取消。

# 批次 J：IPC 错误说明迁出硬编码中文（ADR-106）。
glossary-empty-field = 原词与更正后的写法均为必填。
metrics-range-start-after-end = 开始日期不能晚于结束日期。
metrics-range-format = 日期格式须为 YYYY-MM-DD。
metrics-unavailable = 本地知识度量尚未初始化，请稍后重试。
metrics-vault-unavailable = 尚未打开笔记库。
workspace-metrics-clear-failed = 使用观测尚未初始化，没有可清除的记录。
workspace-metrics-unavailable = 使用观测尚未初始化，请稍后重试。
model-assignment-apply-failed = 分配未生效：算力提供方刷新失败，已恢复为原有分配（{ $error }）。
model-assignment-not-ready = 槽位「{ $slot }」需要 { $capability } 能力，而所选目标当前的状态是 { $readiness }。
model-assignment-rollback-failed = 分配未生效，且原有分配恢复失败（刷新：{ $error }；回滚：{ $rollback }）。请到「模型与算力」重新指定。
model-name-empty = 模型名称不能为空。
model-delete-server-managed = 该模型由服务端托管，无法在本机删除。请在服务端运营控制台管理。
model-install-unsupported = 暂不支持安装该模型：{ $model }。
model-install-server-managed = 该模型由服务端托管，无需也无法在本机安装。
model-plan-not-found-delete = 该删除计划不存在或已被使用，请重新发起。
model-plan-not-found-install = 该安装计划不存在或已被使用，请重新发起。
model-plan-stale-install = 模型、可用磁盘空间或隐私设置已发生变化，请重新确认。
model-plan-stale-assignment = 模型或其分配状态已发生变化，请重新确认。
strict-local-blocked-install = 严格本地模式已开启，联网安装已被阻止。请在「存储与隐私」中关闭后重试。
tidy-list-stale = 预览之后这篇笔记已被修改，请重新发起整理。
export-no-transcript-batch = 所选笔记均无可导出的转写内容。
export-no-text-batch = 所选笔记均无可导出的正文。
export-no-transcript-single = 这篇笔记尚无可导出的转写内容。
export-write-failed = 没有文件被写出，请确认目标目录可写。
diarize-not-applicable-audio = 该录音没有音频分片，无法进行说话人细分。
diarize-not-applicable-media = 该录音没有可播放的媒体分片。
diarize-no-transcript = 该录音尚无转写内容，请先完成转写。
diarize-models-missing = 尚未安装说话人细分模型。请在「设置 → 模型」的「说话人细分」分组下载所需的两个模型后重试。
knowledge-lifecycle-stale = 目标笔记已不存在，或其正文已变化，本次知识维护已放弃。
knowledge-lifecycle-vault-switched = 笔记库已切换，先前的知识维护意图已失效。
knowledge-apply-stale = 这份主题更新计划的来源、主题或范围已变化，或已被你否决，未写入主题；需要重新生成计划。
knowledge-apply-unavailable = 暂时无法应用这份主题更新计划，主题未改动；计划已保留，可稍后重试。
source-grant-unavailable = 没能保存这项授权，录音和转写暂不会作为来源；请稍后重试。
topic-upgrade-batch = 一次最多升级 { $max } 页主题，请分批选择。
translate-no-transcript = 该录音尚无转写内容，请先完成转写。
backend-i18n-unavailable = 后端文案资源尚未初始化。
fetch-media-already-present = 这篇转写已有本地媒体文件。
fetch-no-source-url = 这篇转写没有记录来源地址。
trust-cloud-unavailable = 本构建未包含云客户端。
empty-result-note = 结果为空，没有可存入笔记的内容。
empty-result-lens-note = 结果为空，没有可存入译境笔记的内容。
vault-missing-path-gone = 笔记库路径已不存在：{ $path }
vault-missing-open-failed = 无法打开笔记库：{ $error }
vault-switch-busy = 有任务未能及时暂停，本次切换已中止，请稍后重试。
vault-queue-save-failed = 待办任务未能保存到笔记库，本次切换已中止。请检查磁盘空间和目录权限后重试。
vault-switch-recording = 录音进行中。请先停止录音，再切换笔记库。
empty-asset-download = 下载到的内容为空。
empty-asset-paste = 粘贴的内容为空。
invalid-data-url = 仅支持 base64 编码的 data URL。
invalid-note-path = 笔记路径不合法。
note-not-found = 笔记不存在或超出笔记库范围：{ $path }
not-an-image = 响应内容不是图片（{ $mime }）。
not-a-note = 仅笔记可以在独立窗口中打开。
window-failed = 便签窗口打开失败：{ $error }
route-rejected = 不支持的导航目标：{ $route }
strict-local-blocked-image = 严格本地模式已开启，远程图片下载已被阻止。请在「存储与隐私」中关闭后重试。
url-rejected = 仅支持 http/https 图片链接。
vault-not-registered = 该目录不在笔记库清单中，请先在「管理库」里添加。
vault-window-is-active = 这个笔记库已在主窗口中打开，无需另开窗口。

# 批次 J′：够不着 AppHandle 的那一批（ADR-107）。
vault-switching = 正在切换笔记库，请稍候片刻再试。
vault-not-open = 尚未选择笔记库。请在首次启动引导或设置中选择或新建一个。
workspace-metrics-range-start-after-end = 开始日期不能晚于结束日期。
workspace-metrics-range-format = 日期格式须为 UTC 时区的 YYYY-MM-DD。
vault-window-gone = 该副库窗口已失效，请重新打开。
setting-domain-control-not-a-slot = { $key } 不是模型分配槽。
setting-domain-control-assign = 模型分配必须通过「分配模型」或「清除分配」进行。
setting-domain-control-clear = 清除模型分配必须通过「清除分配」进行。
empty-note = 笔记内容为空，无法保存。
empty-quick-note = 速记内容为空，无法保存。
vault-create-exhausted = 无法生成不重名的笔记路径，请更换名称后重试。
model-load-failed-retranscribe = 二次转写模型加载失败：{ $error }
model-load-failed-asr = 转写模型加载失败。请重新安装，或在设置中选择有效的 whisper 权重：{ $error }
model-missing-realtime-asr = 未找到已就绪的实时转写模型。请在「设置 → 模型」中安装并分配一个。
remote-asr-disabled-realtime = 实时转写已指向服务端，但「服务端转写」开关未开启。请在「设置 → 模型与算力」中开启。
remote-asr-disabled-batch = 离线转写已指向服务端，但「服务端转写」开关未开启。请在「设置 → 模型与算力」中开启。
path-rejected-file = 文件不存在或超出笔记库范围：{ $path }
path-rejected-entry = 条目不存在或超出笔记库范围：{ $path }
url-rejected-external = 仅支持 http/https 外部链接：{ $url }
empty-instruction = 自定义指令不能为空。
invalid-name-empty = 名称不能为空。
invalid-name-separator = 名称不能包含路径分隔符或控制字符。
invalid-name-generic = 该名称无效。
invalid-move-into-self = 不能把一个文件夹移动到它自己里面。
move-target-exists = 目标文件夹里已有同名对象，移动会覆盖它。请先改名或换一个位置。
vault-name-empty = 笔记库名称不能为空。
vault-nested = 该目录与已有笔记库「{ $name }」相互包含，两者的索引与监听会重叠。请另选一个独立目录。
vault-path-invalid-parent-component = 新建笔记库的路径不能包含未解析的上级目录分量（`..`）。
vault-path-invalid = 无法解析笔记库路径：{ $detail }
strict-local-blocked-realtime-asr = 「实时转写」已指向服务端，但当前隐私档是「仅本地」。请更改隐私档，或把该槽位改回本机模型。
strict-local-blocked-batch-asr = 「离线转写」已指向服务端，但当前隐私档是「仅本地」。请更改隐私档，或把该槽位改回本机模型。

# 语音速记 AI 提炼写回正文的待办小标题（PRD-06-058 C′）。
quick-note-refine-todos = 待办
note-ai-invalid = 这次后台 AI 操作的目标笔记或动作已失效，请回到笔记重新发起。

# 创建副本时文件名的后缀（「计划 副本.md」，V5a）。
duplicate-suffix = 副本

# 官方同步（ADR-148）：冲突副本名里的说明，`名字 (冲突副本 2026-10-09).md`。
sync-conflict_copy_label = 冲突副本

# 整库时间点恢复（PRD-13-007）：恢复出来的新文件夹名，时间是所选时刻。
sync-restore-folder-name = { $name }（恢复到 { $time }）
setting-reset-stale = 设置已改变，请重新查看恢复默认的范围。
setting-domain-control-reset = 请使用该设置的专属操作入口。

## 创作空间：新建视频项目的骨架文字（ADR-162，PRD-19-002）
creator-note-research = 研究
creator-note-script = 脚本
creator-section-idea = 选题
creator-section-idea-hint = 一句话说清这期视频讲什么、讲给谁。
creator-section-research = 资料
creator-section-script = 脚本
creator-research-sources = 来源
creator-research-quotes = 引文
creator-material-folder = 素材
creator-research-materials = 素材
creator-script-hook = 开头
creator-script-beat = 节拍 { $n }
creator-script-outro = 结尾

## 创作空间：发布包（ADR-162 ⑪，PRD-19-018~020）
creator-publish-folder = 发布
creator-publish-source-outside = 这篇成片不在这个项目的文件夹里，请重新选择。
creator-not-a-project = 这不是一个视频项目的主页，没法把成片导进来。
creator-bad-date = 计划发布日期要写成「年-月-日」，例如 2026-10-24。
creator-example-title = 示例：笔记本跑大模型
creator-channel-name = 渠道名不能为空，也不能超过 40 个字。
creator-broll-file = 镜头清单
creator-broll-col-index = 序号
creator-broll-col-section = 段落
creator-broll-col-shot = 镜头
creator-broll-col-done = 完成
creator-broll-opening = 开场
creator-broll-yes = 是
creator-project-name = 项目名不能为空，也不能只有文件名里不能用的字符。
creator-project-name-taken = 已经有同名的项目或笔记了，换个名字吧。
creator-source-text = 粘贴的文字不能为空，也不能超过 20 万字；更长的请存成文件再导入。
creator-pasted-title = 粘贴的文字
creator-research-empty = 这条来源没有可读的正文，没法生成。
creator-research-pick = 请选 1 到 { $max } 条来源。
creator-brief-file = 研究简报
creator-brief-based-on = 依据：
creator-brief-separator = 、
creator-brief-overview = 概述
creator-brief-findings = 要点
creator-brief-questions = 待查的问题
creator-research-no-brief = 这个项目还没有研究简报。先在「素材与来源」里写一份研究简报。
creator-topic-text = 选题不能为空，也不能超过 500 字。
creator-script-draft = 脚本初稿
link-import-rights-unconfirmed = 第一次从链接导入之前，请先确认只导入你有权使用的内容。
creator-channel-youtube = YouTube
creator-channel-bilibili = B 站
creator-channel-youtube_shorts = YouTube Shorts
creator-channel-douyin = 抖音
creator-channel-kuaishou = 快手
creator-channel-weixin_channels = 视频号
creator-channel-xiaohongshu_video = 小红书视频
creator-channel-tiktok = TikTok
creator-channel-instagram_reels = Instagram Reels
creator-channel-wechat_article = 公众号文章
creator-channel-xiaohongshu_note = 小红书图文
creator-channel-weibo = 微博
creator-channel-x = X
creator-channel-linkedin = LinkedIn
creator-channel-newsletter = 邮件简报
creator-channel-podcast = 播客
creator-publish-package = { $channel } · 包装
creator-publish-description = 简介
creator-channel-unknown = 这个渠道已经不在渠道列表里了，请刷新发布页签再试。
creator-distribute-blog = 博客
creator-distribute-newsletter = 简报
creator-distribute-thread = 推文串
creator-distribute-shorts = 短视频
creator-distribute-from-script = 脚本
creator-distribute-from-transcript = 成片「{ $title }」的转写
creator-distribute-derived = 由「{ $project }」的{ $source }改写的 AI 草稿，发布前请自己过一遍。
creator-distribute-clip-range = 入点 { $start } · 出点 { $end }
creator-distribute-needs-cut = 挑短视频片段要用成片的转写（有时间点），请先在「发布」页签导入成片。
creator-distribute-no-script = 这个项目还没有写好的脚本，可以改从成片转写改写。
creator-swipe-snippet = 片段
creator-swipe-why = 为什么好
creator-swipe-source = 来源
creator-swipe-pattern-prefix = 套路
creator-swipe-too-few = 至少要有 { $min } 条灵感才能归类。
creator-swipe-bad-name = 「{ $name }」写不成标签，换个名字再试。
creator-quote-dash = ——
app-disabled = 这个功能属于一个还没有安装或已暂时停用的应用。可在「应用」里安装或启用它，库里的内容不受影响。
app-not-installed = 这个应用还没有安装，先在「应用」里安装它。
creator-quote-empty = 选中的这段去掉时间码后没有字，没法存成引文。
creator-quote-no-project = 这篇笔记不属于任何视频项目（不在项目文件夹里，也没和项目主页互相链接），引文没地方存。
creator-publish-chapters = 章节
creator-chapters-parse = AI 这次没有给出可用的章节，请再试一次。
creator-publish-parse = AI 这次没有给出可用的结果，请再试一次。
creator-publish-links = 链接
creator-publish-titles = 标题备选
creator-publish-cover = 封面文字
creator-publish-category = 分区
creator-publish-tags = 标签
creator-publish-sources = 参考来源

## 文档导入（ADR-164）
import-filter-materials = 音视频与文档
doc-import-notice = 导入自 { $file }
doc-import-undecodable = 有些字符解不出来，显示为 �。
doc-import-images-omitted = 有些图片没有带进来，只留了替代文字。
doc-import-table-flattened = 合并单元格或嵌套表格被拆平了。
doc-import-comments = 批注没有带进来。
doc-import-tracked-changes = 修订没有带进来：删掉的文字不保留，新加的照常保留。
doc-import-text-boxes = 文本框里的文字没有带进来。
doc-import-notes = 脚注与尾注没有带进来。
doc-import-table-truncated = 表格共 { $rows } 行、{ $cols } 列，这里只放了前 { $shown_rows } 行、{ $shown_cols } 列；完整数据在原文件里。
doc-import-unsupported = 这种文件还不能导入为文档。
doc-import-too-large = 文件超过 100 MB，没有导入。
doc-import-empty = 文件里没有可以导入的文字。
doc-import-corrupt = 文件打不开：可能已损坏，或者不是它扩展名说的那种格式。
doc-import-sheet-truncated = 工作表「{ $sheet }」共 { $rows } 行、{ $cols } 列，这里只放了前 { $shown_rows } 行、{ $shown_cols } 列；完整数据在原文件里。
doc-import-needs-macos = 旧版 Word（.doc）、RTF 与 ODT 目前只能在 macOS 上导入；请在 Word 里另存为 .docx 后再导入。
doc-import-timeout = 系统转换这个文件超过一分钟还没完成，已停止；文件可能已损坏。请在 Word 里另存为 .docx 后再导入。
doc-import-page = 第 { $page } 页
doc-import-page-separator = 、
doc-import-pages-without-text = 第 { $pages } 页没有可提取的文字（可能是图片或扫描页）。
doc-import-pages-unreadable = 从第 { $from } 页起读不出来（共 { $total } 页），后面的页没有带进来。
doc-import-encrypted = 这份 PDF 设了打开密码，没有导入。请先在预览等应用里去掉密码再导入。
doc-import-no-text-layer = 这份 PDF 几乎没有可提取的文字，看起来是扫描件；识别图片里的文字还不支持，没有导入。
creator-quote-page = 第 { $page } 页
doc-import-read-timeout = 读这个文件超时了：它可能还在云盘（iCloud、OneDrive 等）里没下载到本机。请先在访达里打开或下载它，再重试。
doc-import-read-failed = 读不到这个文件：它可能已被移动、删除，或没有读取权限。
doc-import-mostly-images = 这份 PDF 文字很少，内容可能主要是图片（比如从浏览器打印的网页）；图片里的文字没有带进来。

## 网页剪藏（ADR-164 ⑦）
strict-local-blocked-web-clip = 严格本地模式已开启，网页剪藏已被阻止。请在「存储与隐私」中关闭后重试。
web-clip-disabled = 网页剪藏已在设置里关闭（「存储与隐私 ▸ 网页剪藏」），打开后再试。
web-clip-timeout = 网页 30 秒内没有响应，没有剪藏。请稍后重试。
web-clip-fetch-failed = 打不开这个网页：网络不通，或者网站拒绝了请求。
web-clip-http = 网站返回了错误（HTTP { $status }），没有剪藏。
web-clip-not-html = 这个链接不是网页（可能是 PDF、图片或下载文件），请下载后用「导入素材…」导入。
web-clip-too-large = 网页超过 10 MB，没有剪藏。
web-clip-no-article = 这个网页里认不出正文（可能需要登录，或内容靠脚本加载）。可以在浏览器里「另存为网页」后用「导入素材…」导入。
web-clip-notice = 剪藏自 { $source }
web-clip-author = 作者：{ $author }
web-clip-published = 发布于：{ $published }
web-clip-snapshot = 当时的整页存档：{ $file }
