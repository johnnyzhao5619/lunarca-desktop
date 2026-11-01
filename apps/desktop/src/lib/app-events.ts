// 跨 feature 的 window 事件名常量（纯常量放 lib/，避免 feature 间互相 import 形成循环）。

import type { CitationOpenOutcome, RelocationReceiptDto } from "../ipc/bindings";

/** 引导相关状态可能变了（切库、外部授权回流等）→ 引导 / 横幅重读 Rust `setup_status`。 */
export const ONBOARDING_CHANGED_EVENT = "luna:onboarding-changed";

/**
 * 请求以覆盖层打开首启引导（PRD-14-060 重入）：CustomEvent<string | undefined>，detail =
 * 落到哪一步（缺省 = 第一步）。设置页「重新运行引导」与「完成设置」横幅都发它，由 App 层
 * 唯一宿主接住——引导不再卸载工作区。
 */
export const OPEN_ONBOARDING_EVENT = "luna:onboarding-open";

/** 打开应用级「导出选项」对话框（PRD-11-004/007）：三个导出入口共用同一张。 */
export const EXPORT_OPTIONS_EVENT = "luna:export-options";

export function requestExportOptions(): void {
  window.dispatchEvent(new Event(EXPORT_OPTIONS_EVENT));
}

/**
 * 口播语速校准变了（PRD-19-012：校准 / 恢复默认）→ 打开着脚本的编辑器按新语速重算时长。
 * 校准结果在库内元数据里，编辑器的正文没变，不会自己重算。
 */
export const SCRIPT_RATE_CHANGED_EVENT = "luna:script-rate-changed";

export function notifyScriptRateChanged(): void {
  if (typeof window !== "undefined") {
    window.dispatchEvent(new Event(SCRIPT_RATE_CHANGED_EVENT));
  }
}

/**
 * 大纲拖动调段序（PRD-19-011）：CustomEvent<ScriptSectionMoveDetail>，大纲 → 编辑器。
 * 段落在编辑器的缓冲区里搬（作为一次普通编辑，可撤销），不由大纲直接写盘——那会和用户
 * 正在打的字抢写。只有活跃编辑器认领：同一篇开在两个分栏里时不会被搬两次。
 */
export const SCRIPT_SECTION_MOVE_EVENT = "luna:script-section-move";

export interface ScriptSectionMoveDetail {
  /** 目标笔记相对路径。 */
  path: string;
  /** 被搬那段的二级标题所在行（含 frontmatter，1 起，与大纲同一口径）。 */
  fromLine: number;
  /** 搬到哪一段之前（该段标题所在行）；`null` = 搬到最后。 */
  beforeLine: number | null;
}

export function requestScriptSectionMove(detail: ScriptSectionMoveDetail): void {
  if (typeof window !== "undefined") {
    window.dispatchEvent(new CustomEvent<ScriptSectionMoveDetail>(SCRIPT_SECTION_MOVE_EVENT, { detail }));
  }
}

/**
 * 勾选 / 取消一个 B-roll 镜头（PRD-19-026）：CustomEvent<ScriptBrollToggleDetail>，检查器 → 编辑器。
 * 与搬段同一分工：改的是脚本里那个 `broll` 代码块的围栏行，在编辑器缓冲区里作为一次普通编辑落下
 * （可撤销），检查器不写盘；只有活跃编辑器认领。
 */
export const SCRIPT_BROLL_TOGGLE_EVENT = "luna:script-broll-toggle";

export interface ScriptBrollToggleDetail {
  /** 目标笔记相对路径。 */
  path: string;
  /** 镜头开头围栏所在行（含 frontmatter，1 起，与大纲同一口径）。 */
  line: number;
  done: boolean;
}

export function requestScriptBrollToggle(detail: ScriptBrollToggleDetail): void {
  if (typeof window !== "undefined") {
    window.dispatchEvent(new CustomEvent<ScriptBrollToggleDetail>(SCRIPT_BROLL_TOGGLE_EVENT, { detail }));
  }
}

/**
 * 往一篇脚本的光标处插入一段文字（ADR-162 ⑭：「插入引文」的嵌入）。与搬段同一分工：大纲只发
 * 请求，活跃编辑器作为一次普通编辑落下（可撤销），不由大纲直接写盘。
 */
export const SCRIPT_INSERT_EVENT = "luna:script-insert";

export interface ScriptInsertDetail {
  /** 目标笔记相对路径。 */
  path: string;
  /** 插在光标处的一行。 */
  text: string;
  /** 插完选中它（口述的原话：选中后「整理口述」就在手边）；缺省 = 光标停在插入的字之后。 */
  select?: boolean;
}

export function requestScriptInsert(detail: ScriptInsertDetail): void {
  if (typeof window !== "undefined") {
    window.dispatchEvent(new CustomEvent<ScriptInsertDetail>(SCRIPT_INSERT_EVENT, { detail }));
  }
}

/**
 * 脚本工具栏的「AI 改稿」（ADR-165 ⑦）：对一篇脚本跑一个改稿动作。活跃编辑器认领——有选区就改
 * 选区，没有就先选中光标所在的那一段（二级标题到下一段之前），再打开与选区气泡同一个 AI 面板并
 * 直接开跑；结果照旧逐条审阅、确认才替换。
 */
export const SCRIPT_REWRITE_EVENT = "luna:script-rewrite";

export type ScriptRewriteAction = "script_tighten" | "script_hook" | "script_spoken" | "script_dictation";

export interface ScriptRewriteDetail {
  /** 目标笔记相对路径。 */
  path: string;
  action: ScriptRewriteAction;
}

export function requestScriptRewrite(detail: ScriptRewriteDetail): void {
  if (typeof window !== "undefined") {
    window.dispatchEvent(new CustomEvent<ScriptRewriteDetail>(SCRIPT_REWRITE_EVENT, { detail }));
  }
}

/**
 * 从某一刻开始播放一篇（ADR-162 ⑭：引文出处带时刻的链接）。常驻播放宿主认领；没有宿主的窗口
 * （副库窗）里这一步自然不发生，只打开到那一行。
 */
export const PLAYBACK_AT_EVENT = "luna:playback-at";

export interface PlaybackAtDetail {
  path: string;
  ms: number;
}

export function requestPlaybackAt(detail: PlaybackAtDetail): void {
  if (typeof window !== "undefined") {
    window.dispatchEvent(new CustomEvent<PlaybackAtDetail>(PLAYBACK_AT_EVENT, { detail }));
  }
}

export function requestOnboarding(step?: string): void {
  window.dispatchEvent(new CustomEvent<string | undefined>(OPEN_ONBOARDING_EVENT, { detail: step }));
}

/**
 * 通知已读态变更（历史面板标已读/清空触发）→ 状态栏未读徽标重取计数。
 * 后端标已读命令不发 `notification://event`（那是新通知专用，会误触 toast），
 * 故用轻量 window 事件把「派生的徽标」与「历史面板」的已读态同步，杜绝徽标常驻残值。
 */
export const NOTIFICATIONS_CHANGED_EVENT = "luna:notifications-changed";

/**
 * 编辑器定位（大纲 PRD-06-014 / 搜索命中 PRD-06-031 → 编辑器）：
 * CustomEvent<EditorRevealDetail>。编辑器实例自行按 path 匹配响应（多 pane 同笔记各自
 * 滚动；面板与 pane 树解耦）。目标笔记尚未加载时请求会被暂存，加载完成即兑现——
 * 搜索命中常常是"先打开这篇、再跳到那一行"，两件事天然不同步。
 */
export const EDITOR_REVEAL_EVENT = "luna:editor-reveal";

export interface EditorRevealDetail {
  /** 目标笔记相对路径。 */
  path: string;
  /** 原文 1-based 行号（Live Preview / 源码模式光标定位）。 */
  line: number;
  /**
   * 正文第几个标题（0-based；阅读模式按序滚动锚点）。
   * `null` = 定位目标不是标题（如搜索命中的普通行）——阅读模式没有源码行的概念，
   * 此时只打开笔记、不滚动，而不是假装滚到某处。
   */
  headingIndex: number | null;
  /**
   * 定位后是否把焦点交给编辑器。跳过去接着写的入口（搜索命中、链接、深链、大纲）都是 `true`；
   * 视频脚本大纲里可搬的段名行是 `false`——焦点留在那一行，⌥↑ / ⌥↓ 才搬的是段而不是
   * 编辑器里光标所在的那一行（PRD-19-011）。
   */
  focusEditor: boolean;
}

/**
 * 就地加词（PRD-07-061）：CustomEvent<GlossaryAddDetail>。
 *
 * 用户是**在看见错字的那一刻**才知道自己需要一条词条——转写把「大威」认成别的、
 * 语音输入把人名打错。让他记住"设置 → 个人词典"再翻回来手打一遍，等于把这条需求
 * 丢在原地。故凡是可选中文本的内容面（笔记正文、回放字幕），选中后右键即可加词，
 * 由唯一宿主 `GlossaryQuickAdd` 接住并预填。
 */
export const GLOSSARY_ADD_EVENT = "luna:glossary-add";

export interface GlossaryAddDetail {
  /** 预填到「原始词 / 误识形式」的文本（用户选中的那段）。 */
  term: string;
}

/**
 * AI 关系标签重扫请求（PRD-06-035b/035c）：CustomEvent<GraphRescanRequestDetail>。
 *
 * 定向重扫的入口在各文件树的文件夹右键（对象在哪儿、动作就在哪儿），而确认卡与
 * 进行中状态由唯一宿主（`useGraphController`，实例在 WorkspaceShell）承载——各列表
 * 只派发这个事件，不必知道确认卡在哪儿，与就地加词同一模式。
 */
export const GRAPH_RESCAN_EVENT = "luna:graph-rescan";

export interface GraphRescanRequestDetail {
  /** 重扫范围：Vault 相对文件夹路径；`null` = 整个库。 */
  scope: string | null;
  /**
   * 确认卡里哪一档是主动作；缺省 = 「只补缺口」。
   *
   * 「标签语言分裂」那条建议必须给 `"force"`：不一致的笔记指纹是最新的，「只补缺口」
   * 按指纹跳过、一篇都扫不到（PRD-06-035d ②）。派发方知道自己为什么要重扫，宿主不该猜。
   */
  emphasis?: "gaps" | "force";
}

/**
 * 批量整理知识笔记请求（PRD-06-044 批量入口）：CustomEvent<DistillBatchRequestDetail>。
 *
 * 与 `GRAPH_RESCAN_EVENT` 同一模式——入口在对象所在处（各清单的文件夹右键），
 * 确认卡与进行中状态由唯一常驻宿主承载，派发方不必知道卡在哪儿。
 */
export const DISTILL_BATCH_EVENT = "luna:distill-batch";

export interface DistillBatchRequestDetail {
  /** 整理范围：Vault 相对文件夹路径；`null` = 整个库。 */
  scope: string | null;
}

/**
 * 「把**这几篇**整理成知识笔记」请求（PRD-06-044 / PRD-06-054）：右键单篇与多选走同一条。
 *
 * **结果也归那个唯一宿主**：命令返回的是"进了几件、哪些没进、为什么"，而"被挡下的
 * 那几篇怎么呈现"已经有一处实现（批量结果卡，逐篇指名道姓 + 理由）。派发方自己发命令、
 * 把结果压成状态栏一行的代价，是用户只看到「已加入 2 件；1 件未加入」却不知道是哪一篇、
 * 也不知道下一步该做什么（2026-09-02 真机：选中 3 篇、进队列 2 篇）。
 */
export const DISTILL_NOTES_EVENT = "luna:distill-notes";

export interface DistillNotesDetail {
  /** 要整理的笔记（Vault 相对路径，按所见顺序）。 */
  paths: string[];
}

/**
 * 「把这几篇合成一篇知识笔记」请求（PRD-06-047）：CustomEvent<MergeNotesDetail>。
 *
 * 与批量整理同一模式（入口在对象所在处、执行由常驻宿主承载），但产出形态不同：
 * 这一条是"一堆碎片合成一篇"，故走独立事件而非复用批量那条。
 *
 * **载荷是真实 paths，不是文件夹范围**（ADR-087 / TDD-07 §2.1）：入口此前是
 * `QuickNotes/` 的文件夹右键，于是"哪些算速记"由目录回答——而速记会被移动、普通笔记
 * 也会被标记为待整理，目录从来不等于成熟度。资格仍由 Rust 判（`merge_notes`
 * 入队时校验一次、执行时按同一份快照重验）。
 *
 * 宿主只负责把它**排进处理队列**（`ProcTaskKind::Merge`）：运行态归队列真值，完成由
 * 知识写入通知承接——与逐篇整理同一条反馈通道，不另养第二处运行态。
 */
export const MERGE_NOTES_EVENT = "luna:merge-notes";

export interface MergeNotesDetail {
  /** 要合成一篇的笔记（Vault 相对路径）——Rust 已判定可作为汇总源的那些。 */
  paths: string[];
}

/**
 * 蒸馏重生成的差异预览请求（PRD-06-045）：CustomEvent<DistillPreviewRequestDetail>。
 *
 * 与批量整理同一模式——入口在对象所在处（页头徽标 / 右键菜单），对话框由唯一常驻宿主
 * 承载；派发方不必知道它在哪儿，也不必自己管"生成中/已就绪/落盘中"这三态。
 */
export const DISTILL_PREVIEW_EVENT = "luna:distill-preview";

export interface DistillPreviewRequestDetail {
  /** 源资料笔记相对路径。 */
  notePath: string;
  /**
   * 本次显式指定的模板键（`undefined` = 不指定，由 Rust 按「已记录 > 建议」解析）。
   *
   * 「换个模板重新整理」对**编辑过**的笔记同样只经预览这一条路（PRD-06-045），故参数
   * 随请求一路带到 `distill_preview`——它也是预览槽命中判据的一部分。
   */
  template?: string;
  /** 本次显式指定的「附整理稿」开关（`undefined` = 不指定）。 */
  fullText?: boolean;
}

/**
 * 「把这个标签下的笔记汇总成一篇」请求（PRD-06-047）：CustomEvent<TagMergeDetail>。
 *
 * 与「多选汇总」共用同一执行宿主与同一种队列任务（`ProcTaskKind::Merge`，来源政策由
 * `MergeOrigin` 区分）——两者产出形态完全一致（一堆笔记 → 一篇），只是候选集口径
 * 不同，没有理由各管各的状态。
 */
export const TAG_MERGE_EVENT = "luna:tag-merge";

export interface TagMergeDetail {
  /** 标签路径（含子孙笔记，与标签视图过滤语义同源）。 */
  tag: string;
}

/**
 * 打开知识体检报告（PRD-16-030/032）：无 detail 的通知事件。
 *
 * 与归入确认卡同一模式——入口散在动机产生地（图谱控制面的维护区、启动概览的体检行、
 * 体检通知的深链），报告面板由唯一常驻宿主承载。派发方不必知道面板在哪儿，也不必自己
 * 管「取数 / 深度体检进行中 / 忽略后重取」这三件事。
 */
export const CHECKUP_EVENT = "luna:checkup";

/** 体检请求的载荷：`focusPath` = 只看涉及这一页的发现（`null` = 全库视角）。 */
export interface CheckupDetail {
  focusPath: string | null;
}

/**
 * 请求打开体检报告。
 *
 * **`focusPath` 是一个视角，不是第二条命令**（TDD-07 §4.1d）：体检命令的作用域恒为
 * 全库（PRD-16-030/031），「针对本页体检」= 打开这份报告并按该页路径过滤**既有**发现。
 * 为它加一条单页体检命令，就是给同一份判定造第二个入口，而两个入口迟早对同一页给出
 * 不同的发现数。
 */
export function requestKnowledgeCheckup(focusPath: string | null = null): void {
  if (typeof window !== "undefined") {
    window.dispatchEvent(new CustomEvent<CheckupDetail>(CHECKUP_EVENT, { detail: { focusPath } }));
  }
}

/**
 * 让文件树**显示记录**（PRD-06-055 待沉淀积压卡的处置动作）：无 detail 的通知事件。
 *
 * 「隐藏记录」是文件树实例内的视图态（会话内记忆、不持久化，PRD-06-052）——把它提成
 * 全局状态只为了让一张卡能关掉它，代价远大于收益。故沿用本文件既有的那一套：动机产生
 * 地（体检卡）派发，唯一持有该状态的地方（`NoteTree`）响应。
 *
 * **它只掀开视图，绝不替用户沉淀任何一篇**（PRD-06-053：把草稿判成知识只有用户有
 * 资格）。卡片主按钮同时把最旧的那一篇打开，于是页头那两枚沉淀入口就在他眼前。
 */
export const SHOW_DRAFTS_EVENT = "luna:show-drafts";

/** 请求文件树取消「隐藏记录」。 */
export function requestShowDrafts(): void {
  if (typeof window !== "undefined") {
    window.dispatchEvent(new CustomEvent(SHOW_DRAFTS_EVENT));
  }
}

/**
 * 打开某篇笔记的版本历史（PRD-16-025 建议条的「查看变更」）：
 * CustomEvent<VersionHistoryRequestDetail>。
 *
 * Auto 档已经直写完的那几页，用户要问的是「改了什么」——而那个答案本就躺在每次写盘
 * 自动留下的版本快照里（PRD-06-040）。故不另造一套"归入变更记录"（那会是第二份事实源），
 * 只是把既有的版本历史面板从别处叫开。编辑器实例按 path 自行匹配响应，与
 * `EDITOR_REVEAL_EVENT` 同一模式。
 */
export const VERSION_HISTORY_EVENT = "luna:version-history";

export interface VersionHistoryRequestDetail {
  /** 目标笔记相对路径。 */
  path: string;
}

/**
 * 「对某篇笔记做 X」的请求必须**暂存**而不能只靠事件：`dispatchEvent` 是同步的，
 * 而「打开那篇笔记」要等 React 重渲染——目标页尚未打开时，事件在监听器按 `pathRef`
 * 匹配的那一刻派发进真空。暂存 + 编辑器加载完成时兑现才是完整的一对。
 *
 * 这个形状有两个用户（版本历史的「查看变更」、编辑器定位 reveal），收敛为一个工厂——
 * 各写一份的下场是下一个"对笔记做 X"的事件又把竞态重演一遍。
 * 带过期时间：一条没被兑现的旧请求不该在几分钟后用户自己点开那篇笔记时突然生效。
 */
function makePendingSlot<T>(ttlMs: number) {
  let slot: { key: string; value: T; at: number } | null = null;
  return {
    set(key: string, value: T, now: number): void {
      slot = { key, value, at: now };
    },
    /** 命中未过期的暂存即消费（一次性——多 pane 同篇只有先加载完的那个兑现）。 */
    consume(key: string, now: number): T | null {
      if (!slot) {
        return null;
      }
      if (now - slot.at > ttlMs) {
        slot = null;
        return null;
      }
      if (slot.key !== key) {
        return null;
      }
      const value = slot.value;
      slot = null;
      return value;
    },
  };
}

/** 暂存请求的有效期（毫秒）：覆盖"打开标签页 + 读盘"的最慢情形即可。 */
const PENDING_REQUEST_TTL_MS = 10_000;

const pendingVersionHistory = makePendingSlot<true>(PENDING_REQUEST_TTL_MS);
const pendingEditorReveal = makePendingSlot<EditorRevealDetail>(PENDING_REQUEST_TTL_MS);

/** 请求打开某篇笔记的版本历史（暂存 + 广播，两个消费面缺一不可）。 */
export function requestVersionHistory(path: string, now: number = Date.now()): void {
  pendingVersionHistory.set(path, true, now);
  // 暂存是本函数的本体，事件只是"已打开的编辑器立即响应"的加速面——无头环境（单测）
  // 没有 window 也不该炸，暂存照常成立。
  if (typeof window !== "undefined") {
    window.dispatchEvent(
      new CustomEvent<VersionHistoryRequestDetail>(VERSION_HISTORY_EVENT, { detail: { path } }),
    );
  }
}

/** 编辑器侧兑现：自己的路径命中未过期的暂存请求即消费（返回 `true` = 该开面板了）。 */
export function consumePendingVersionHistory(path: string, now: number = Date.now()): boolean {
  return pendingVersionHistory.consume(path, now) === true;
}

/**
 * 请求编辑器定位（搜索命中/通知深链的「先打开这篇、再跳到那一行」）。
 *
 * 与版本历史同一竞态、同一修法：目标笔记未打开时，`EDITOR_REVEAL_EVENT` 的监听器
 * 在派发那一刻 `pathRef` 还是旧值——纯事件派发必然丢失，症状是"点了命中只打开笔记、
 * 不跳到那一行"。此前 reveal 只有"路径已匹配但内容未加载"的**实例级**暂存，
 * 盖不住"根本还没打开"这一幕。
 */
export function requestEditorReveal(detail: EditorRevealDetail, now: number = Date.now()): void {
  pendingEditorReveal.set(detail.path, detail, now);
  if (typeof window !== "undefined") {
    window.dispatchEvent(new CustomEvent<EditorRevealDetail>(EDITOR_REVEAL_EVENT, { detail }));
  }
}

/** 编辑器侧兑现定位请求（加载完成处调用；`null` = 没有属于这篇的未过期请求）。 */
export function consumePendingEditorReveal(
  path: string,
  now: number = Date.now(),
): EditorRevealDetail | null {
  return pendingEditorReveal.consume(path, now);
}

// ─── 编辑器作用域命令（命令面板 → 活跃编辑器，PRD-07-041 / TDD-07 §4.2）────────────
//
// 命令面板住在 shell，而「切视图 / 朗读 / 版本历史」这些动作的状态归当前活跃的编辑器
// （多 pane、独立窗各持一份）。与 `⌘E` 同一分工：shell 只广播「执行哪条」，每个编辑器
// 按自己的 `active` 认领——不暂存：命令面板打开时那个编辑器已经在了，没有"先打开再兑现"
// 的竞态。

export const EDITOR_COMMAND_EVENT = "luna:editor-command";

export interface EditorCommandDetail {
  id: string;
}

export function requestEditorCommand(id: string): void {
  if (typeof window !== "undefined") {
    window.dispatchEvent(new CustomEvent<EditorCommandDetail>(EDITOR_COMMAND_EVENT, { detail: { id } }));
  }
}

/**
 * 出处打开后的定位终态（PRD-16-012「失效显式化」）：CustomEvent<CitationOutcomeDetail>。
 * `open_citation` / `open_sealed_plan_source` 在 Rust 内完成导航，发起的视图随即被换走，
 * 故终态交给常驻主窗口的 `CitationOutcomeHost`。对话出处与主题计划出处共用这一条。
 */
export const CITATION_OUTCOME_EVENT = "luna:citation-outcome";

export interface CitationOutcomeDetail {
  title: string;
  outcome: CitationOpenOutcome | "error";
}

/** 命中原处（exact）不打扰；其余终态交给宿主提示。 */
export function reportCitationOutcome(detail: CitationOutcomeDetail): void {
  if (detail.outcome === "exact") return;
  window.dispatchEvent(new CustomEvent<CitationOutcomeDetail>(CITATION_OUTCOME_EVENT, { detail }));
}

/**
 * 改名 / 移动的回执（PRD-06-030 ③）：CustomEvent<RelocationReceiptDto>。
 *
 * 入口散在文件树、对象菜单、文件夹清单与编辑器标题；回执条只有主窗工作区一个宿主
 * （它同时管把标签页映射回去）。派发方不必知道条子在哪儿。
 */
export const RELOCATION_RECEIPT_EVENT = "luna:relocation-receipt";

/** 报一次改名 / 移动的回执；没有可撤销的凭据（名字没变、一项都没搬成）时不报。 */
export function announceRelocation(receipt: RelocationReceiptDto): void {
  if (typeof window !== "undefined" && receipt.undo.length > 0) {
    window.dispatchEvent(new CustomEvent<RelocationReceiptDto>(RELOCATION_RECEIPT_EVENT, { detail: receipt }));
  }
}
