import { useCallback, useEffect, useRef, useState } from "react";
import { IconButton } from "../../components/ui/IconButton";
import { useVaultReload } from "../../hooks/use-vault-reload";
import { useAppEnabled, useNoteTypeToolsAvailable } from "../../hooks/use-apps";
import { scriptSectionRangeAt } from "./script-section-range";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { defaultKeymap, history, historyKeymap, undo } from "@codemirror/commands";
import { closeBrackets, closeBracketsKeymap } from "@codemirror/autocomplete";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { noteStem } from "../../lib/note-path";
import { EDITOR_MARKDOWN_EXTENSIONS } from "../../lib/markdown-dialect";
import { foldEffect, syntaxTree } from "@codemirror/language";
import { Compartment, EditorState, Text, Transaction, type Extension } from "@codemirror/state";
import { EditorView, drawSelection, highlightActiveLine, keymap, placeholder, tooltips } from "@codemirror/view";
import { search, searchKeymap } from "@codemirror/search";
import { useTranslation } from "react-i18next";
import {
  commands,
  events,
  type CaptureDestinationDto,
  type EditorViewModeDto,
  type FloatingMenuLanding,
  type FloatingMenuPendingDto,
  type FloatingMenuScope,
  type TidyListPreviewDto,
} from "../../ipc/bindings";
import {
  consumePendingEditorReveal,
  consumePendingVersionHistory,
  EDITOR_REVEAL_EVENT,
  VERSION_HISTORY_EVENT,
  type EditorRevealDetail,
  type VersionHistoryRequestDetail,
  EDITOR_COMMAND_EVENT,
  type EditorCommandDetail,
  announceRelocation,
} from "../../lib/app-events";
import { applyIncrementalText, insertLink, toggleBold, toggleItalic } from "./markdown-format";
import {
  cueLineHighlightExtension,
  timecodeGutterExtension,
  timecodeLinesExtension,
  setTimecodeLinesEffect,
  editorProductivityExtensions,
  highlightCueLineEffect,
  markdownCloseBracketsData,
  selectionWordCount,
  noteWordCount,
  frontmatterFoldingExtension,
  frontmatterLinesExtension,
  lunaScaffoldHidingExtension,
  getFrontmatterRange,
  resetEditorHistory,
  saveEditorHistory,
  EditorHistoryCache,
} from "./editor-extensions";
import { resetWikiResolutions, wikiLinkClickHandler, wikiLinkExtension, wikiTargetAtElement } from "./wiki-link";
import { openWikiLink, resolveWikiTargets } from "./wiki-link-open";
import { mdLinkClickHandler, mdLinkTargetAtElement } from "./md-link";
import { isSafeExternalUrl } from "../../lib/markdown";
import { DistillBadge } from "../distill/DistillBadge";
import { PlaybackEntryBadge, type PlaybackSection } from "../playback/PlaybackEntryBadge";
import { usePlaybackHostOptional } from "../playback/playback-host";
import { activeTimecodeIndex, useTimecodeLines } from "../playback/use-timecode-lines";
import { QuickNoteProductBar } from "./QuickNoteProductBar";
import { DraftBadge } from "../knowledge/DraftBadge";
import { useNoteDraftState } from "../knowledge/use-note-draft-state";
import { lunaSyntaxHighlighting } from "../../lib/markdown-highlight";
import { inlineTagExtension, taskCheckboxExtension } from "./markdown-decorations";
import { livePreviewExtension } from "./live-preview";
import { codeBlockLinesExtension } from "./code-block-lines";
import { blockSpacingExtension } from "./block-spacing";
import { listIndentExtension } from "./list-indent";
import { listEditingKeymap } from "./list-editing";
import { selectionLayerExtension } from "./selection-layer";
import { multiCursorExtension } from "./multi-cursor";
import { unresolvedLinkExtension } from "./reference-links";
import { codeFenceEditingKeymap } from "./code-fence-editing";
import { codeLanguageForFence } from "../../lib/code-highlight";
import { tableCellTargetOf, tablePreviewExtension, type TableMenuTarget } from "./table-preview";
import { tableMenuItems, tableMenuLabels } from "./table-menu";
import { tableAt } from "./table-model";
import type { TableAiTask } from "./FloatingMenu";
import { copyTableAs } from "./table-clipboard";
import { tableEditingKeymap } from "./table-edit";
import { tablePickerExtension } from "./table-picker";
import { blockMoveKeymap } from "./block-move";
import { headingFoldExtension } from "./heading-fold";
import { richPreviewExtension } from "./rich-preview";
import type { TablePastePreview } from "./table-edit";
import { pasteTableCell } from "./table-paste";
import { rebaseCreatedDraft, serializedSave } from "./save-session";
import { ChangeSet } from "@codemirror/state";
import { subscribeTabFocusMode, tabFocusModeEnabled, toggleTabFocusMode } from "../../lib/tab-focus-mode";
import { formatStateAt, setHeading, type FormatState, type HeadingLevel } from "./format-commands";
import { typewriterScrolling } from "./writing-modes";
import { recheckSpelling, spellcheckExtension, spellingTargetAt } from "./spellcheck";
import type { SpellingMenu } from "./editor-actions";
import {
  editorContextMenuItems,
  runEditorFormatAction,
  type EditorFormatAction,
} from "./editor-actions";
import { useContextMenu } from "../../components/ContextMenuHost";
import { glossaryTermFromSelection } from "../settings/glossary-term";
import { requestGlossaryAdd } from "../settings/GlossaryQuickAdd";
import { FormatToolbar } from "./FormatToolbar";
import { SelectionBubble } from "./SelectionBubble";
import { AiLandedToast } from "./AiLandedToast";
import { completionsExtension } from "./completions";
import { VersionHistoryPanel } from "./VersionHistoryPanel";
import { TidyListDialog } from "./TidyListDialog";
import { FloatingMenu, type FloatingMenuControls } from "./FloatingMenu";
import { bubbleAnchorBox, type SelectionBox } from "./floating-menu-position";
import {
  aiRangeOf,
  aiSessionExtension,
  aiSessionKeymap,
  contextAround,
  flashLandedRange,
  setAiRange,
  textBeforeCursor,
  type AiSessionKeyHandlers,
} from "./ai-session";
import { ReadingView } from "./ReadingView";
import { respondsToEditorShortcuts, toggledViewMode } from "./view-mode";
import {
  EDITOR_KEYED_SETTING_KEYS,
  codeMirrorKey,
  editorCommandForKey,
  editorShortcut,
  isEditorSaveShortcut,
} from "../command/command-registry";
import { useShortcutBindings } from "../../hooks/use-shortcut-bindings";
import { DoubleTapDetector } from "../../lib/double-tap";
import { ReadAloudControls } from "./ReadAloudControls";
import { ttsReadingHighlight, setTtsReadingRange } from "./tts-highlight";
import { useTts, useTtsAvailable } from "../tts/use-tts";
import { dropAt, pasteAssetsExtension, pasteFromClipboard, type PasteAssetsContext, type PasteConversionChoice } from "./paste-assets";
import { EDITOR_FILE_DROP_EVENT, type EditorFileDrop } from "../../lib/editor-file-drop";
import type { HtmlPasteDto, LineDiffDto, NoteTypeDto, ScriptOutlineDto, TemplateFileDto } from "../../ipc/bindings";
import { TemplateInsertDialog } from "./TemplateInsertDialog";
import { PastePreviewDialog } from "./PastePreviewDialog";
import { DiffView } from "../../components/ui/DiffView";
import { ConflictCopyBanner } from "./ConflictCopyBanner";
import { outlineFromState, type OutlineHeading } from "./outline";
import { afterEditorsSaved, registerEditorSave, withEditorSaveGuard } from "./editor-save-guard";
import { usePrompt } from "../../components/PromptHost";

import {
  BookOpenIcon,
  ClockIcon,
  CodeIcon,
  EllipsisIcon,
  FolderIcon,
  PencilIcon,
  SparklesIcon,
  SpeakIcon,
  SpeakingIcon,
} from "../../components/ui/Icon";
import { buildMenu, separator } from "../../lib/context-menu";
import { formatShortcut } from "../../lib/shortcut-display";
import { useBoxSize, useInlineSize } from "../../hooks/use-resize-observer";
import { EDITOR_TIER_WIDE_MIN, editorTierFor } from "./editor-tier";
import { vaultErrorText } from "../../lib/vault-error";
import { goTo, openNote } from "../../lib/navigate";
import { readingHeadings } from "./reading-view";
import { LinkPreviewPopover, linkRefFromDataset, useLinkPreview, type LinkRef } from "./LinkPreview";
import { isImeKeyEvent } from "../../lib/ime-key-event";
import { isMetadataOnlyWrite } from "./metadata-write";
import {
  SCRIPT_INSERT_EVENT,
  SCRIPT_REWRITE_EVENT,
  type ScriptRewriteAction,
  type ScriptRewriteDetail,
  SCRIPT_RATE_CHANGED_EVENT,
  SCRIPT_SECTION_MOVE_EVENT,
  SCRIPT_BROLL_TOGGLE_EVENT,
  type ScriptInsertDetail,
  type ScriptSectionMoveDetail,
  type ScriptBrollToggleDetail,
} from "../../lib/app-events";

/** `selectedWords`：选中文字的字数（多光标合计，B6）；没选中为 null。 */
/** `outline` 为 `undefined` = 这一篇的大纲还没算出来（装载中），不是「没有标题」。 */
/** `loadFailed`：这一篇没装载成功——字数与「已保存」都不成立，状态栏不报。 */
/** `script`：视频脚本（`type: script`）的分段与口播时长（Rust 算，PRD-19-011/012）；别的类型为 null。 */
export interface EditorMetrics { wordCount: number; selectedWords: number | null; dirty: boolean; path: string | null; outline: OutlineHeading[] | undefined; wordGoal: number | null; loadFailed: boolean; script: ScriptOutlineDto | null }
export const EMPTY_EDITOR_METRICS: EditorMetrics = { wordCount: 0, selectedWords: null, dirty: false, path: null, outline: undefined, wordGoal: null, loadFailed: false, script: null };

interface EditorSelectionState {
  from: number;
  to: number;
  text: string;
}

/**
 * AI 会话（PRD-06-020/021）：面板打开时冻结的作用域 / 输入文本 / 范围 + 锚定包围盒。
 * 范围的**权威**是 `ai-session.ts` 里随文档变更映射的 StateField，这里的 from/to 只是
 * 打开那一刻的快照（兜底）。
 */
interface AiSessionState {
  scope: FloatingMenuScope;
  text: string;
  from: number;
  to: number;
  /** 选区前后段落的原样切片（选区作用域）；Rust 决定取不取。 */
  context: { before: string; after: string } | null;
  /** 选区 / 光标包围盒（相对编辑器容器）；`null` = 顶部停靠（整篇模式）。 */
  box: SelectionBox | null;
  /** 从气泡「相关笔记」进来：打开即执行库内搜索。 */
  initialSearch: boolean;
  /** 后台跑完、等着落地的结果（PRD-06-021）；`null` = 这是一次前台发起。 */
  pending: FloatingMenuPendingDto | null;
  /** 从表格菜单打开的表格 AI 任务（对标方案 T6）；`null` = 普通召唤。 */
  tableTask?: TableAiTask | null;
  /** 打开即开跑的动作（脚本工具栏的「AI 改稿」，ADR-165 ⑦）；`null` = 等用户选。 */
  autoRun?: ScriptRewriteAction | null;
}

/** 选区气泡条：选区稳定后出现，记下范围以便滚动时重算包围盒。 */
interface BubbleState {
  from: number;
  to: number;
  box: SelectionBox;
}

/** 落地反馈（PRD-06-021）：底部「已替换 / 已插入 · 撤销」横条。 */
interface LandedState {
  landing: FloatingMenuLanding;
}

/** 落地高亮停留时长（毫秒），略长于 CSS 渐隐动画。 */
const LANDED_HIGHLIGHT_MS = 2_600;

/** 停手多久后再数字数、重建大纲（对标方案 A5：长文档每键扫全文的那几项）。 */
const DOCUMENT_FACTS_DELAY_MS = 200;

/** 已保存正文的 CM `Text` 形态（脏判定用 `Text.eq`，同一份基线只切一次行）。 */
let savedTextCache: { source: string; text: Text } = { source: "", text: Text.empty };
function savedTextOf(source: string): Text {
  if (savedTextCache.source !== source) savedTextCache = { source, text: Text.of(source.split("\n")) };
  return savedTextCache.text;
}

/** 选区 / 光标的包围盒（相对编辑器容器）；文档位置量不到坐标时返回 `null`。 */
function selectionBoxFor(
  view: EditorView,
  section: HTMLElement,
  from: number,
  to: number,
): SelectionBox | null {
  const docLength = view.state.doc.length;
  const start = view.coordsAtPos(Math.min(from, docLength));
  const end = view.coordsAtPos(Math.min(to, docLength), -1);
  if (!start || !end) {
    return null;
  }
  const rect = section.getBoundingClientRect();
  return {
    left: Math.min(start.left, end.left) - rect.left,
    right: Math.max(start.right, end.right) - rect.left,
    top: Math.min(start.top, end.top) - rect.top,
    bottom: Math.max(start.bottom, end.bottom) - rect.top,
  };
}

/** 气泡条的锚定盒：跨行选区只取首行（`bubbleAnchorBox`）；AI 面板仍用整个选区的包围盒。 */
function bubbleBoxFor(
  view: EditorView,
  section: HTMLElement,
  from: number,
  to: number,
): SelectionBox | null {
  const docLength = view.state.doc.length;
  const start = view.coordsAtPos(Math.min(from, docLength));
  const end = view.coordsAtPos(Math.min(to, docLength), -1);
  if (!start || !end) {
    return null;
  }
  const rect = section.getBoundingClientRect();
  const content = view.contentDOM.getBoundingClientRect();
  const paddingRight = Number.parseFloat(getComputedStyle(view.contentDOM).paddingRight) || 0;
  const local = (r: { left: number; right: number; top: number; bottom: number }) => ({
    left: r.left - rect.left,
    right: r.right - rect.left,
    top: r.top - rect.top,
    bottom: r.bottom - rect.top,
  });
  return bubbleAnchorBox(local(start), local(end), content.right - paddingRight - rect.left);
}

export interface VaultEditorProps {
  path: string | null;
  onStatus: (status: string) => void;
  /** 保存产生/变更了路径：草稿落盘 `previousPath = null`；重命名 = 原路径（供标签重映射）。 */
  onSavedPath?: (path: string, previousPath?: string | null) => void;
  onOpenPath?: (path: string) => void;
  /** 正文内联 `#tag` 点击（跳标签视图，PRD-06-033）。 */
  onOpenTag?: (tag: string) => void;
  onEditorMetrics?: (metrics: EditorMetrics) => void;
  autoFocusTitle?: boolean;
  onTitleFocused?: () => void;
  folderStats?: { files: number; folders: number } | null;
  /**
   * 本编辑器是不是**当前活跃的那一个**——编辑器作用域快捷键（`⌘E`）据此让位。
   *
   * 多 pane 时每个 pane 各挂一份 `window` 监听，不判活跃就会一次按键切换所有 pane。
   * 缺省 `true`：一窗一编辑器的宿主（独立笔记窗、副库窗口、语音速记）没有"另一个
   * 编辑器"可让，让它们逐个传一个恒真的值只是噪音。
   */
  active?: boolean;
  /**
   * 视图模式（PRD-06-017：Live Preview / 源码 / 阅读）。受控用法（多标签下模式是
   * 标签级状态）；缺省时组件内部自持（单编辑器视图，如语音速记）。
   */
  viewMode?: EditorViewModeDto;
  onViewModeChange?: (mode: EditorViewModeDto) => void;
  /**
   * 独立窗口场景（PRD-07-036 弹出窗口）：窗口关闭前在 flush 之后、真正关闭之前调用
   * （回主窗打开等收尾）。提供时，即使无未保存改动也拦截关闭以执行该收尾。
   */
  onWindowClose?: () => Promise<void> | void;
  /**
   * 副库窗口的受限模式（PRD-06-050）：隐藏 AI 面板 / 蒸馏与资料徽标 / 图谱标签 /
   * 版本历史——AI 一切写回以主库为落点，这些入口在副库语境下是跨库数据污染面，
   * 不是"少个功能"。编辑 / 重命名 / wiki 创建照常（命令已按窗口身份路由）。
   */
  restricted?: boolean;
  /**
   * 库上下文（PRD-06-050）：`note://written` 跨窗同步按此过滤——路径是库相对的，
   * 主窗与副库窗可能同时开着**不同库**的同名路径。缺省 `"main"`（主库语境）。
   */
  vaultContext?: string;
  /**
   * 这块画布长在**哪个目的地**里（PRD-06-058 P0-1）——只在 `path === null`（空白画布）
   * 时有意义：新建出来的东西落成什么形态、以及正文占位与空态该说哪一套话。
   *
   * **交出去的是事实、不是判定**：本组件把它原样传给 `create_note_from_text`，落盘
   * 形态全由 Rust 决定（ADR-003）。缺省 `"notes"` = 笔记库那张桌子；独立笔记窗与副库
   * 窗口永远带着路径进来，没有空白画布可落，故它们不传。
   */
  canvasDestination?: CaptureDestinationDto;
}

export function VaultEditor({
  path,
  onStatus,
  onSavedPath,
  onOpenPath,
  onOpenTag,
  onEditorMetrics,
  autoFocusTitle,
  onTitleFocused,
  folderStats,
  active = true,
  viewMode,
  onViewModeChange,
  restricted = false,
  vaultContext = "main",
  canvasDestination = "notes",
  onWindowClose,
}: VaultEditorProps) {
  const { t, i18n } = useTranslation("editor");
  // 右键菜单跨命名空间取词（编辑器条目 + `common:menu.*` 那份全应用共用的剪贴板四项）。
  const { t: tMenu } = useTranslation(["editor", "common", "settings", "workspace"]);
  const prompt = usePrompt();
  const contextMenu = useContextMenu();
  // 文本朗读（TTS，TDD-05 §9）：单一发声者，本编辑器发起的会话记 `readSessionId` 认领。
  const {
    state: ttsState,
    speak: ttsSpeak,
    pause: ttsPause,
    resume: ttsResume,
    stop: ttsStop,
  } = useTts();
  const ttsAvailable = useTtsAvailable();
  const hostRef = useRef<HTMLDivElement | null>(null);
  const viewRef = useRef<EditorView | null>(null);
  const pathRef = useRef<string | null>(null);
  const savedContentRef = useRef("");
  const documentContentRef = useRef("");
  // 字数 / 大纲的延后计算（每次按键都扫全文，长文档输入就卡）。
  const documentFactsTimerRef = useRef<number | null>(null);
  const dirtyRef = useRef(false);
  const saveActionRef = useRef<() => Promise<boolean>>(async () => true);
  const flushRef = useRef(serializedSave(() => saveActionRef.current(), () => dirtyRef.current));
  useEffect(() => registerEditorSave(() => flushRef.current(), (held) => {
    if (sectionRef.current) sectionRef.current.inert = held;
  }), []);
  const externalContentRef = useRef<string | null>(null);
  const externalPendingRef = useRef(false);
  const savingContentRef = useRef<string | null>(null);
  const draftEditsRef = useRef<ChangeSet | null>(null);
  const pasteResolveRef = useRef<((value: PasteConversionChoice) => void) | null>(null);
  /** 编辑器整篇动作的键位（⌘E / ⌘J，W3b 起可改键）：真值与双击时间窗，按键判定经 ref 读此刻的。 */
  const editorKeyState = useShortcutBindings(EDITOR_KEYED_SETTING_KEYS);
  const editorKeyStateRef = useRef(editorKeyState);
  editorKeyStateRef.current = editorKeyState;
  const editorDoubleTapRef = useRef(new DoubleTapDetector());
  const readingShortcut = editorKeyState.bindings["shortcuts.toggle_reading_view"] ?? "";
  const aiShortcut = editorKeyState.bindings["shortcuts.ai_composer"] ?? "";
  /** 「插入模板」选择框（V6）：开着时持有它的 resolve，选中给路径、关掉给 `null`。 */
  const [templateDialogOpen, setTemplateDialogOpen] = useState(false);
  const templateResolveRef = useRef<((template: TemplateFileDto | null) => void) | null>(null);
  const [pastePreview, setPastePreview] = useState<{ preview: HtmlPasteDto; plain: string; tableTarget?: TablePastePreview } | null>(null);
  const [conflictDiff, setConflictDiff] = useState<LineDiffDto | null>(null);
  const conflictCopyBusyRef = useRef(false);
  const nameInputRef = useRef<HTMLInputElement | null>(null);
  /** 已把哪条笔记的内容灌进编辑器（定位请求要等它与目标一致才能兑现）。 */
  const loadedPathRef = useRef<string | null>(null);
  /** 目标尚未加载完时暂存的定位请求（加载完成即兑现，随后清空）。 */
  const revealPendingRef = useRef<EditorRevealDetail | null>(null);
  /**
   * 空白画布所属目的地的最新值（PRD-06-058 P0-1）。
   *
   * 落盘发生在 `flushRef` 里，那个闭包不随本 prop 重建；读 prop 会拿到挂载那一刻的旧值，
   * 而"落盘落错了目的地"恰恰是不会报错、只有事后翻文件树才发现的那类缺陷。
   */
  const canvasDestinationRef = useRef(canvasDestination);
  canvasDestinationRef.current = canvasDestination;
  const [wordCount, setWordCount] = useState(0);
  /** 这一篇的字数目标（W6b，`luna.word_goal`，由 Rust 读出）；`null` = 没设。 */
  const [wordGoal, setWordGoal] = useState<number | null>(null);
  const [selectedWords, setSelectedWords] = useState<number | null>(null);
  const selectionFactsTimerRef = useRef<number | null>(null);
  const [liveOutline, setLiveOutline] = useState<OutlineHeading[] | undefined>(undefined);
  /** 视频脚本的分段时长（只对 `type: script` 取；判定与估算都在 Rust）。 */
  const [liveScript, setLiveScript] = useState<ScriptOutlineDto | null>(null);
  const liveScriptRef = useRef<ScriptOutlineDto | null>(null);
  liveScriptRef.current = liveScript;
  /** 这一篇的笔记类型（装载时从 `meta.note_type` 取）：决定要不要算脚本时长。 */
  const noteTypeRef = useRef<NoteTypeDto | null>(null);
  /** 同一个事实的渲染面：浮动菜单要按它决定有没有「脚本 ▸」。 */
  const [noteType, setNoteType] = useState<NoteTypeDto | null>(null);
  /** 只认最后一次请求的回答：快速连敲时旧的时长不能盖过新的。 */
  const scriptRequestRef = useRef(0);
  /**
   * 脚本的时长与工具归「创作」应用（ADR-165 ④）：应用停用时不算时长，大纲与状态栏里的
   * 脚本工具随之消失；「存为引文」同理。
   */
  // 固定问「脚本」这一类型：按当前篇的类型取，切到脚本的那一拍会沿用上一篇的答案。
  const scriptToolsAvailable = useNoteTypeToolsAvailable("script");
  const scriptToolsAvailableRef = useRef(scriptToolsAvailable);
  scriptToolsAvailableRef.current = scriptToolsAvailable;
  const quoteAvailable = useAppEnabled("creator");
  const [dirty, setDirty] = useState(false);
  const [externalPending, setExternalPending] = useState(false);
  // 从挂载那一刻就是这篇的名字：经 `notes://` 打开会请编辑器聚焦标题（`autoFocusTitle`），而笔记
  // 空间从别的空间切回来时是新挂载——草稿若从空串起步，聚焦又失焦的那一下就会以「笔记名称不能为空」
  // 报错（ADR-168 A7 真机发现：从「应用 › 模板」用它新建后横幅常驻）。
  const [nameDraft, setNameDraft] = useState(() => displayNameFromPath(path));
  const [renameError, setRenameError] = useState<string | null>(null);
  const [autosaveEnabled, setAutosaveEnabled] = useState(false);
  /** 粘贴远程图片链接是否下载本地化（editor.localize_remote_images）；粘贴时懒读，默认开。 */
  const localizeRemoteImagesRef = useRef(true);
  const [autosaveDebounceMs, setAutosaveDebounceMs] = useState<number | null>(null);
  const [versionHistoryOpen, setVersionHistoryOpen] = useState(false);
  /** AI 关系标签手动生成中（PRD-06-035；防重复点击，完成后状态栏反馈）。 */
  const [graphTagsBusy, setGraphTagsBusy] = useState(false);
  /** 「整理列表层级」预览（PRD-06-056）：非 null 即确认卡开着；改动清单来自 Rust。 */
  const [tidyPreview, setTidyPreview] = useState<TidyListPreviewDto | null>(null);
  /**
   * 「标记为记录」（PRD-06-058 D）能不能给——与页头「记录」徽标**同一份取数**
   * （`useNoteDraftState`），故两者永远说得上话：徽标在场时这一项必然不在。
   */
  const draftState = useNoteDraftState(restricted ? null : (path ?? null));

  // ── 就地听（PRD-04-066 / ADR-118）──────────────────────────────────────────
  //
  // 编辑器不拥有播放器，只订阅常驻宿主：时间戳行可点即跳、当前句随播放高亮。副库窗口
  // 没有宿主（`null`），这一整块随之不出现——它是增益，不是编辑器的主线。
  const playbackHost = usePlaybackHostOptional();
  /** 这一篇是不是录音/转写：由页头那枚入口探测后上报，编辑器不另探一次。 */
  const [playbackSection, setPlaybackSection] = useState<PlaybackSection | null>(null);
  const timecodeLines = useTimecodeLines(
    restricted ? null : (path ?? null),
    playbackHost !== null && playbackSection !== null,
  );
  /** 宿主此刻放的是不是**这一篇**——放着别篇时本篇不该跟着高亮。 */
  const playingThisNote = playbackHost?.noteRel === path && path !== null;
  /** 装订线播放键的文案（扩展只在构建时取一次回调，故经 ref 拿最新的语言）。 */
  const timecodePlayLabelRef = useRef("");
  timecodePlayLabelRef.current = t("timecode.playFromHere");
  // 登记"我开着哪一篇"：宿主据此决定「跟随」开关要不要出现在「正在播放」条上。
  const registerEditorNote = playbackHost?.registerEditorNote;
  useEffect(() => {
    if (!registerEditorNote) return;
    registerEditorNote(restricted ? null : (path ?? null));
    return () => registerEditorNote(null);
  }, [registerEditorNote, restricted, path]);

  const timecodeSeekRef = useRef<(ms: number) => void>(() => {});
  timecodeSeekRef.current = (ms: number) => {
    if (!playbackHost || !path) return;
    // 跳转随起播意图一起交给宿主：宿主此刻可能装着别篇，`open()` 返回时还没换源，
    // 这里自己 seek 会打在旧媒体上、随后被换源归零（真机 2026-09-08：点时间戳行从头起播）。
    playbackHost.open(path, { play: true, seekMs: ms });
  };
  const [selectionState, setSelectionState] = useState<EditorSelectionState | null>(null);
  /** 光标处格式态（工具栏激活高亮 / 右键菜单勾选），随选区/文档变化刷新。 */
  const [formatState, setFormatState] = useState<FormatState>({
    strong: false,
    em: false,
    del: false,
    code: false,
    heading: 0,
    bullet: false,
    ordered: false,
    task: false,
    quote: false,
  });
  /** 写作视图模式（PRD-06-014，Typora 对标）：设置持久化，toggle 写回设置。 */
  const [typewriterMode, setTypewriterMode] = useState(false);
  /** 西文拼写检查（W6a，设置 `editor.spellcheck`）。 */
  const [spellcheck, setSpellcheck] = useState(false);
  const [focusMode, setFocusMode] = useState(false);
  /** 格式工具栏显隐（editor.show_format_toolbar，设备级；页头「更多」与设置页双入口）。 */
  const [showFormatToolbar, setShowFormatToolbar] = useState(true);
  /** 朗读语速百分比（speech.rate）与逐句高亮开关（speech.highlight）；设置为事实源。 */
  const [speechRatePct, setSpeechRatePct] = useState(100);
  const [speechHighlight, setSpeechHighlight] = useState(true);
  /** 本编辑器发起的朗读会话 id（0 = 无）。用于认领全局单一发声者、判定控制条与高亮归属。 */
  const [readSessionId, setReadSessionId] = useState(0);
  const readSessionRef = useRef(0);
  /** 朗读区间基准偏移：全文=0，选区=选区起点（源区间 + 基准 = 文档位置）。 */
  const readBaseRef = useRef(0);
  /** 打字机滚动扩展插槽（随设置热切换，不重建编辑器实例）。 */
  const typewriterCompartmentRef = useRef(new Compartment());
  const spellcheckCompartmentRef = useRef(new Compartment());
  const localeCompartmentRef = useRef(new Compartment());
  const historyCompartmentRef = useRef(new Compartment());
  /**
   * 这一篇没能装载（读取失败，例如还在 iCloud 上没下载，TDD-06 §4.1 ②）：编辑区清空并只读。
   * 不清空的话上一篇的正文留在原地、标题却是这一篇——用户一改，自动保存就把上一篇的内容
   * 写进这一篇的路径（V3c 真机发现）。下次装载成功即解除。
   */
  const loadGateCompartmentRef = useRef(new Compartment());
  const loadFailedRef = useRef(false);
  // 没装载成功时编辑区里说清楚（只读的空白编辑面 +「开始输入…」会让人以为这篇是空的）；
  // 「重试」递增 `loadAttempt` 让装载 effect 再跑一次。
  const [loadError, setLoadError] = useState<string | null>(null);
  const [loadAttempt, setLoadAttempt] = useState(0);
  // 切走的那几篇的撤销历史：一个编辑器跨标签复用，切回来要能接着 ⌘Z（PRD-06-014）。
  const historyCacheRef = useRef(new EditorHistoryCache());
  const [aiSession, setAiSession] = useState<AiSessionState | null>(null);
  const [pointerDown, setPointerDown] = useState(false);
  const [bubble, setBubble] = useState<BubbleState | null>(null);
  /** `editor.floating_context`：改写时是否附带选区前后段落（设置为事实源）。 */
  const [contextEnabled, setContextEnabled] = useState(true);
  const [landed, setLanded] = useState<LandedState | null>(null);
  /** 面板把「接受 / Esc」两个键位处理器交到这里，编辑器 keymap 经它转发。 */
  const aiControlsRef = useRef<FloatingMenuControls | null>(null);
  const sectionRef = useRef<HTMLElement | null>(null);
  const isFolder = path !== null && !path.endsWith(".md");

  // ── 视图模式（PRD-06-017）────────────────────────────────────────────────────
  // 受控（多标签：模式随标签持久化）或内部自持（单编辑器视图）二选一。
  const [internalMode, setInternalMode] = useState<EditorViewModeDto>("live");
  const mode = viewMode ?? internalMode;
  const changeMode = onViewModeChange ?? setInternalMode;
  const modeRef = useRef(mode);
  modeRef.current = mode;
  const changeModeRef = useRef(changeMode);
  changeModeRef.current = changeMode;
  /** 最近一次编辑型模式（live/source）；Cmd+E 从阅读切回时恢复。 */
  const lastEditModeRef = useRef<EditorViewModeDto>("live");
  if (mode !== "reading") {
    lastEditModeRef.current = mode;
  }
  /** Live Preview 装饰组热切换插槽（reconfigure 不重建编辑器实例）。 */
  const presentationCompartmentRef = useRef(new Compartment());
  /**
   * 时间戳装订线的隔间（PRD-04-066）。
   *
   * **它必须能整个摘掉**，不能只是"没有标记的空装订线"：CM6 只要装了 gutter 扩展就
   * 一定渲染那一列容器，于是**没有时间戳的笔记**（速记、普通笔记）左侧凭空多出一条竖的
   * 白边——真机 2026-09-08 上报的"切速记窗口中出现预期外的白框"就是它。
   */
  const timecodeCompartmentRef = useRef(new Compartment());
  /** 阅读模式渲染触发器：文档变化（外部重载/AI 落地）时也刷新只读视图。 */
  const [docVersion, setDocVersion] = useState(0);

  /**
   * 兑现一条定位请求；目标内容尚未灌进编辑器时返回 `false`（调用方暂存待兑现）。
   * 只读 ref，故依赖为空数组即正确。
   */
  const applyReveal = useCallback((detail: EditorRevealDetail): boolean => {
    if (loadedPathRef.current !== detail.path) {
      return false;
    }
    if (modeRef.current === "reading") {
      if (detail.headingIndex === null) {
        return true;
      }
      const container = sectionRef.current?.querySelector(".luna-editor__reading-scroll");
      const target = container ? readingHeadings(container)[detail.headingIndex] : undefined;
      if (!target) {
        return false;
      }
      target.scrollIntoView({ block: "start", behavior: "smooth" });
      return true;
    }
    const view = viewRef.current;
    if (!view) {
      return false;
    }
    const lineNo = Math.max(1, Math.min(detail.line, view.state.doc.lines));
    const line = view.state.doc.line(lineNo);
    view.dispatch({
      selection: { anchor: line.from },
      effects: EditorView.scrollIntoView(line.from, { y: "start" }),
    });
    if (detail.focusEditor) view.focus();
    return true;
  }, []);

  useEffect(() => {
    if (autoFocusTitle && nameInputRef.current) {
      nameInputRef.current.focus();
      nameInputRef.current.select();
      onTitleFocused?.();
    }
  }, [autoFocusTitle, path, onTitleFocused]);

  useEffect(() => {
    onEditorMetrics?.({ wordCount, selectedWords, dirty, path: pathRef.current, outline: liveOutline, wordGoal, loadFailed: loadError !== null, script: liveScript });
  }, [dirty, liveOutline, liveScript, loadError, onEditorMetrics, selectedWords, wordCount, wordGoal]);

  function displayNameFromPath(nextPath: string | null): string {
    if (!nextPath) {
      return "";
    }
    const fileName = nextPath.split("/").pop() ?? nextPath;
    return fileName.endsWith(".md") ? fileName.slice(0, -3) : fileName;
  }

  /** 编辑器里此刻的正文（编辑器在就以它为准；`documentContentRef` 只是没有编辑器时的兜底）。 */
  function currentDocumentText(): string {
    return viewRef.current?.state.doc.toString() ?? documentContentRef.current;
  }

  /** 视频脚本：按此刻的全文重算分段时长（不是脚本就清空）。 */
  function refreshScriptOutline(content: string) {
    const request = ++scriptRequestRef.current;
    if (noteTypeRef.current !== "script" || !scriptToolsAvailableRef.current) {
      setLiveScript(null);
      return;
    }
    void commands.scriptOutline(content).then((outline) => {
      if (request !== scriptRequestRef.current) return;
      setLiveScript((previous) => (JSON.stringify(previous) === JSON.stringify(outline) ? previous : outline));
    });
  }

  // 创作应用启用 / 停用：正文没变，脚本时长与工具按新状态出现或收回。
  useEffect(() => {
    refreshScriptOutline(currentDocumentText());
  }, [scriptToolsAvailable]);

  // 语速校准变了（大纲里的「用我的录音校准 / 恢复默认」）：正文没变，按新语速重算一次。
  useEffect(() => {
    const onRateChanged = () => refreshScriptOutline(currentDocumentText());
    window.addEventListener(SCRIPT_RATE_CHANGED_EVENT, onRateChanged);
    return () => window.removeEventListener(SCRIPT_RATE_CHANGED_EVENT, onRateChanged);
  });

  function scheduleDocumentFacts(view: EditorView) {
    if (documentFactsTimerRef.current !== null) window.clearTimeout(documentFactsTimerRef.current);
    documentFactsTimerRef.current = window.setTimeout(() => {
      documentFactsTimerRef.current = null;
      if (!view.dom.isConnected) return;
      const content = view.state.doc.toString();
      documentContentRef.current = content;
      setWordCount(noteWordCount(content));
      const outline = outlineFromState(view.state, getFrontmatterRange(view.state)?.to ?? 0);
      setLiveOutline((previous) => JSON.stringify(previous) === JSON.stringify(outline) ? previous : outline);
      refreshScriptOutline(content);
    }, DOCUMENT_FACTS_DELAY_MS);
  }

  function markDirty(nextDirty: boolean) {
    dirtyRef.current = nextDirty;
    setDirty(nextDirty);
  }

  function updateSelectionState(view: EditorView) {
    setFormatState(formatStateAt(view.state));
    // 选中字数：取消选择立刻清掉；拖选过程中停手一拍再数（选中整篇长文时逐帧数会拖慢拖选）。
    if (selectionFactsTimerRef.current !== null) window.clearTimeout(selectionFactsTimerRef.current);
    selectionFactsTimerRef.current = null;
    if (view.state.selection.ranges.every((r) => r.empty)) {
      setSelectedWords(null);
    } else {
      selectionFactsTimerRef.current = window.setTimeout(() => {
        selectionFactsTimerRef.current = null;
        if (view.dom.isConnected) setSelectedWords(selectionWordCount(view.state));
      }, DOCUMENT_FACTS_DELAY_MS);
    }
    const range = view.state.selection.main;
    if (range.empty) {
      setSelectionState(null);
      return;
    }
    const from = Math.min(range.from, range.to);
    const to = Math.max(range.from, range.to);
    const text = view.state.sliceDoc(from, to);
    setSelectionState(text.trim().length > 0 ? { from, to, text } : null);
  }

  // ── 格式动作（右键菜单 / 格式工具栏共用，PRD-06-014）────────────────────────

  function runFormatAction(action: EditorFormatAction) {
    const view = viewRef.current;
    if (!view) {
      return;
    }
    void runEditorFormatAction(view, action, {
      linkPlaceholder: t("linkPlaceholder"),
      // 「插入图片 / 附件」：对话框与复制落盘都在 Rust；失败走状态栏通道（过程消息，不灌稳态）。
      pickAttachments: async () => {
        const notePath = pathRef.current;
        if (!notePath) {
          onStatus(t("paste.saveFirst"));
          return [];
        }
        onStatus(t("paste.saving"));
        const res = await commands.importAssetFiles(notePath);
        if (res.status !== "ok") {
          onStatus(vaultErrorText(res.error) || t("paste.failed"));
          return [];
        }
        onStatus("");
        return res.data;
      },
      // 「插入模板…」：选择框只给选中的那份，变量与 `{{cursor}}` 由 Rust 渲染（V6）。
      pickTemplate: async () => {
        const template = await new Promise<TemplateFileDto | null>((resolve) => {
          templateResolveRef.current = resolve;
          setTemplateDialogOpen(true);
        });
        if (template === null) {
          return null;
        }
        const res = await commands.renderTemplateForInsert(template, pathRef.current);
        if (res.status !== "ok") {
          onStatus(vaultErrorText(res.error));
          return null;
        }
        return res.data;
      },
      copyText: async (text) => {
        await commands.popupCopy(text);
      },
      convertTextToTable: async (text) => {
        const res = await commands.editorTextToTable(text);
        const table = res.status === "ok" ? res.data : null;
        if (table === null) onStatus(t("table.noColumns"));
        return table;
      },
      paste: (convert) => pasteFromClipboard(view, pasteContext(), convert ? "markdown" : "automatic"),
    });
  }

  function resolveTemplate(template: TemplateFileDto | null) {
    const resolve = templateResolveRef.current;
    templateResolveRef.current = null;
    setTemplateDialogOpen(false);
    resolve?.(template);
  }

  function resolvePaste(value: PasteConversionChoice) {
    const resolve = pasteResolveRef.current;
    pasteResolveRef.current = null;
    setPastePreview(null);
    resolve?.(value);
  }

  function pasteContext(): PasteAssetsContext {
    return { getNotePath: () => pathRef.current,
      ensureNotePath: async () => (await flushRef.current()) ? pathRef.current : null,
      localizeRemoteImages: () => localizeRemoteImagesRef.current,
      richTextEnabled: () => modeRef.current === "live",
      onStatus,
      // 失败说原因（同上方图片调整处）：严格本地模式拦下远程图片时，Rust 给的那句「已阻止、去哪关」
      // 此前被一句笼统的「附件保存失败」盖掉（E5 真机 2026-10-06）。
      messages: { saving: t("paste.saving"), saveFirst: t("paste.saveFirst"), failed: (error) => vaultErrorText(error) || t("paste.failed"),
        downloading: t("paste.downloading"), converting: t("paste.converting"), cancelled: t("paste.cancelled"), conversionFailed: t("paste.conversionFailed"), readFailed: t("paste.readFailed") },
      confirmConversion: (preview, plain, tableTarget) => new Promise((resolve) => {
        pasteResolveRef.current?.(null);
        pasteResolveRef.current = resolve;
        setPastePreview({ preview, plain, tableTarget });
      }),
    };
  }

  useEffect(() => () => { pasteResolveRef.current?.(null); pasteResolveRef.current = null; }, []);
  useEffect(() => { resolvePaste(null); setConflictDiff(null); setSelectedWords(null); }, [path]);
  useEffect(() => () => {
    if (selectionFactsTimerRef.current !== null) window.clearTimeout(selectionFactsTimerRef.current);
  }, []);

  /** 表格菜单（单元格右键 / 行列把手）：条目与动作归 `table-menu.ts`，这里只注入文案与剪贴板。 */
  function openTableMenu(event: MouseEvent | React.MouseEvent, target: TableMenuTarget, appendToTextField = false) {
    const view = viewRef.current;
    const notePath = pathRef.current;
    if (!view || !notePath) return;
    const items = tableMenuItems({
      view,
      target,
      locale: i18n.language,
      labels: tableMenuLabels(t),
      onCopy: (source, format) => {
        void copyTableAs(source, format, notePath).then((ok) => {
          if (!ok) onStatus(t("error"));
        });
      },
      onAi: restricted ? undefined : (task) => openTableAiSession(target, task),
    });
    contextMenu.open(event, items, { appendToTextField, highlight: null });
  }

  function openEditorContextMenu(event: React.MouseEvent) {
    const view = viewRef.current;
    if (!view || isFolder || !path || modeRef.current === "reading") {
      // 不认领：交回宿主兜底（阅读态给「复制选区」，PRD-07-059）。
      return;
    }
    // 右键落在表格单元格里：剪切 / 复制 / 粘贴之后接表格菜单（插行删列、对齐、排序……）。
    const cellTarget = tableCellTargetOf(event.target);
    if (cellTarget) {
      openTableMenu(event, cellTarget, true);
      return;
    }
    // 原生语义：右键落在选区外 → 光标移到点击处；落在选区内 → 保留选区。
    const pos = view.posAtCoords({ x: event.clientX, y: event.clientY });
    if (pos !== null) {
      const sel = view.state.selection.main;
      if (pos < sel.from || pos > sel.to) {
        view.dispatch({ selection: { anchor: pos } });
      }
    }
    // 拼写建议（W6a）：要先问 Rust 这个词拼没拼错，菜单得等一下。原生菜单此刻就拦下，
    // 取到结果后在同一坐标打开（宿主不收已被拦下的事件，故换一个同坐标的事件对象）。
    const target = spellcheck && pos !== null ? spellingTargetAt(view.state, pos) : null;
    if (target) {
      event.preventDefault();
      const { clientX, clientY } = event;
      void commands.spellingSuggestions(target.word).then((result) => {
        const spelling: SpellingMenu | null = result && {
          guesses: result.guesses,
          onReplace: (guess) => {
            const current = viewRef.current;
            // 等待期间正文变了（这个词已不在原处）就不替换，免得改错地方。
            if (!current || current.state.sliceDoc(target.from, target.to) !== target.word) return;
            current.dispatch({
              changes: { from: target.from, to: target.to, insert: guess },
              userEvent: "input.spelling",
            });
            current.focus();
          },
          // 学进词典后正文没变，但判定变了：要求立刻重查一次，波浪线才会消失。
          onLearn: () =>
            void commands.learnSpelling(target.word).then(() => {
              viewRef.current?.dispatch({ effects: recheckSpelling.of(null) });
            }),
        };
        showEditorContextMenu(new MouseEvent("contextmenu", { clientX, clientY }), spelling);
      });
      return;
    }
    showEditorContextMenu(event, null);
  }

  function showEditorContextMenu(event: React.MouseEvent | MouseEvent, spelling: SpellingMenu | null) {
    const view = viewRef.current;
    if (!view) return;
    const state = formatStateAt(view.state);
    setFormatState(state);
    const selection = view.state.selection.main;
    contextMenu.open(
      event,
      editorContextMenuItems({
        spelling,
        t: tMenu,
        formatState: state,
        // 选区取**此刻的编辑器状态**而非 React 里那份异步 state：右键前刚点了一下的
        // 光标移动尚未回流，用旧值会把「复制」错误地置灰。
        hasSelection: !selection.empty,
        onAction: runFormatAction,
        onAiActions: openAiComposerFromMenu,
        onReadSelection: readSelectionAloud,
        // 词条同样取此刻的编辑器状态（理由同上）。
        glossaryTerm: glossaryTermFromSelection(view.state.sliceDoc(selection.from, selection.to)),
        onAddToGlossary: () => {
          requestGlossaryAdd(view.state.sliceDoc(selection.from, selection.to).trim());
        },
        onTidyListLevels: () => {
          void openTidyListLevels();
        },
        // 不能标记时**不渲染该项**（`null`），而不是渲染一个点不动的禁用项。
        onMarkAsDraft:
          draftState.markableAsDraft && path
            ? () => {
                void commands.markNoteAsDraft(path).then((result) => {
                  if (result.status === "error") {
                    // 拒绝有明确原因（资料 / 主题页 / 知识地图），原样回流。
                    onStatus(result.error);
                    return;
                  }
                  void draftState.refresh();
                });
              }
            : null,
        // 模板属主库：副库窗口里不给（V6）。
        templatesAvailable: !restricted,
        aiShortcut,
      }),
      // 归属对象是光标处而非某个可高亮的行；高亮整片编辑面会把正文糊上一层底色。
      { highlight: null },
    );
  }

  // ── 字数目标（W6b）────────────────────────────────────────────────────────
  // 填的是原文，解析（全角、千分位、0 = 清掉、上限）在 Rust。Rust 从磁盘读改写 `luna:` 块，
  // 所以先把这一篇存盘（同改名的保存屏障）。
  async function promptWordGoal() {
    const currentPath = pathRef.current;
    if (!currentPath) return;
    const input = await prompt({
      title: t("wordGoal.title"),
      description: wordGoal === null ? t("wordGoal.desc") : t("wordGoal.descCurrent", { goal: wordGoal }),
      label: t("wordGoal.label"),
      value: wordGoal === null ? "" : String(wordGoal),
      confirmLabel: t("wordGoal.confirm"),
    });
    if (input === null) return;
    const result = await afterEditorsSaved(
      () => commands.setWordGoal(currentPath, input),
      () => ({ status: "error" as const, error: t("wordGoal.saveFirst") }),
    );
    if (result.status === "error") {
      onStatus(vaultErrorText(result.error));
      return;
    }
    if (result.data.kind === "invalid") {
      onStatus(t("wordGoal.invalid"));
    } else if (result.data.kind === "set") {
      setWordGoal(result.data.goal);
      onStatus(t("wordGoal.set", { goal: result.data.goal }));
    } else {
      setWordGoal(null);
      onStatus(t("wordGoal.cleared"));
    }
  }

  // ── AI 会话（PRD-06-020/021）──────────────────────────────────────────────
  // 选区稳定（鼠标松开 + 短暂停顿）后只出**气泡条**；AI 面板由气泡上的 ✦ 或 ⌘J 召唤：
  // 有选区 = 选区作用域（锚定选区）、无选区 = 光标处、页头 ✦ = 整篇（顶部停靠）。
  // 结果进行中 / 待落地时冻结（选区变化不顶替面板）。冻结范围交给 `ai-session.ts` 的
  // StateField 随文档变更映射——等待期间在上方编辑，替换仍命中原来那段。

  function openSelectionSession(
    selection: EditorSelectionState,
    options: { search?: boolean; tableTask?: TableAiTask; autoRun?: ScriptRewriteAction } = {},
  ) {
    const view = viewRef.current;
    const section = sectionRef.current;
    if (!view || !section) {
      return;
    }
    view.dispatch({ effects: setAiRange.of({ from: selection.from, to: selection.to }) });
    setBubble(null);
    setAiSession({
      scope: "selection",
      text: selection.text,
      from: selection.from,
      to: selection.to,
      context: contextAround(view.state.doc, selection.from, selection.to),
      box: selectionBoxFor(view, section, selection.from, selection.to),
      initialSearch: options.search ?? false,
      pending: null,
      tableTask: options.tableTask ?? null,
      autoRun: options.autoRun ?? null,
    });
  }

  /** 表格菜单的 AI 项：以整张表的源码为选区作用域打开面板（副库窗口没有 AI，PRD-06-050）。 */
  function openTableAiSession(target: TableMenuTarget, task: TableAiTask) {
    const view = viewRef.current;
    if (!view || restricted) return;
    const model = tableAt(view.state, target.tableFrom);
    if (!model) return;
    openSelectionSession({ from: model.from, to: model.to, text: view.state.sliceDoc(model.from, model.to) }, { tableTask: task });
  }

  function openCursorSession() {
    const view = viewRef.current;
    const section = sectionRef.current;
    if (!view || !section) {
      return;
    }
    const pos = view.state.selection.main.head;
    view.dispatch({ effects: setAiRange.of({ from: pos, to: pos }) });
    setBubble(null);
    setAiSession({
      scope: "cursor",
      text: textBeforeCursor(view.state.doc, pos),
      from: pos,
      to: pos,
      context: null,
      box: selectionBoxFor(view, section, pos, pos),
      initialSearch: false,
      pending: null,
    });
  }

  function openDocSession(pending: FloatingMenuPendingDto | null = null) {
    const view = viewRef.current;
    if (!view) {
      return;
    }
    const text = view.state.doc.toString();
    if (!text.trim()) {
      return;
    }
    view.dispatch({ effects: setAiRange.of(null) });
    setBubble(null);
    setAiSession({
      scope: "doc",
      text,
      from: 0,
      to: view.state.doc.length,
      context: null,
      box: null,
      initialSearch: false,
      pending,
    });
  }

  /**
   * 这一篇有没有后台跑完、等着落地的结果（PRD-06-021）。有就把面板开出来呈现它——
   * 完成通知的深链落到笔记，用户点进来看到的必须是那份结果，而不是一扇空面板。
   */
  function openPendingSessionIfAny(notePath: string) {
    if (restricted || isFolder) {
      return;
    }
    void commands.floatingMenuPending(notePath).then((pending) => {
      if (pending && pathRef.current === notePath) {
        openDocSession(pending);
      }
    });
  }

  /** 右键菜单入口：恒为「打开」（菜单本身已把面板关了，再 toggle 会关两次）。 */
  function openAiComposerFromMenu() {
    if (restricted || isFolder || modeRef.current === "reading") {
      return;
    }
    const view = viewRef.current;
    const selection = view?.state.selection.main;
    if (view && selection && !selection.empty) {
      const from = Math.min(selection.from, selection.to);
      const to = Math.max(selection.from, selection.to);
      const text = view.state.sliceDoc(from, to);
      if (text.trim().length > 0) {
        openSelectionSession({ from, to, text });
        return;
      }
    }
    openCursorSession();
  }

  /** ⌘J / 命令面板：开着就关，否则按有无选区打开对应作用域。 */
  function toggleAiComposer() {
    if (restricted || isFolder || modeRef.current === "reading") {
      return;
    }
    if (aiSession) {
      closeAiSession();
      return;
    }
    if (selectionState) {
      openSelectionSession(selectionState);
      return;
    }
    openCursorSession();
  }

  function closeAiSession() {
    setAiSession(null);
    const view = viewRef.current;
    if (view) {
      view.dispatch({ effects: setAiRange.of(null) });
      view.focus();
    }
  }

  function toggleContextSetting() {
    const next = !contextEnabled;
    setContextEnabled(next);
    void commands.setSetting("editor.floating_context", { kind: "bool", value: next });
  }

  /** 在关系图谱中查看当前笔记（graph:// 深链 → 图谱视图局部聚焦，PRD-06-034）。 */
  function openInGraph() {
    const currentPath = pathRef.current;
    if (!currentPath) {
      return;
    }
    // 通道选择归 `goTo`（它自己问"这扇窗里有人管导航吗"）——这里此前有一个
    // `onOpenInGraph` prop，存在的唯一理由就是让调用方替它选通道。
    goTo(`graph://${encodeURIComponent(currentPath)}`);
  }

  /** AI 关系标签生成核心（PRD-06-035，头部按钮与 AI 操作面板两入口共用）：先落盘保证
   *  生成对象是最新正文，再强制重生成；标签写入经 note://written 回流本编辑器。 */
  async function runGraphTags(): Promise<
    { ok: true; tags: string[] } | { ok: false; error: string }
  > {
    if (dirtyRef.current && !(await flushRef.current())) {
      return { ok: false, error: t("graphTags.saveFailed") };
    }
    const currentPath = pathRef.current;
    if (!currentPath) {
      return { ok: false, error: t("graphTags.saveFailed") };
    }
    const result = await commands.noteGraphTags(currentPath);
    if (result.status === "ok") {
      return { ok: true, tags: result.data };
    }
    return { ok: false, error: result.error };
  }

  /** 「整理列表层级」预览（PRD-06-056）：先落盘保证归一对象是最新正文（`runGraphTags`
   *  同一纪律），Rust 只读计算改动清单；0 行轻提示「无需整理」，有改动才开确认卡。 */
  async function openTidyListLevels() {
    if (dirtyRef.current && !(await flushRef.current())) {
      onStatus(t("error"));
      return;
    }
    const currentPath = pathRef.current;
    if (!currentPath) {
      return;
    }
    const result = await commands.tidyListLevelsPreview(currentPath);
    if (result.status !== "ok") {
      onStatus(t("error"));
      return;
    }
    if (result.data.changes.length === 0) {
      onStatus(t("tidyList.nothing"));
      return;
    }
    setTidyPreview(result.data);
  }

  /** 确认卡点了确认：应用命令凭预览指纹拒绝陈旧确认（Rust 侧校验），写盘走既有
   *  写入咽喉（快照/索引/`note://written`），新正文经事件回流本编辑器。 */
  async function applyTidyListLevels() {
    const preview = tidyPreview;
    const currentPath = pathRef.current;
    setTidyPreview(null);
    if (!preview || !currentPath) {
      return;
    }
    const result = await commands.tidyListLevelsApply(currentPath, preview.content_hash);
    if (result.status === "ok") {
      onStatus(t("tidyList.done", { count: result.data }));
    } else {
      onStatus(t("error"));
    }
  }

  /** 头部图标入口：核心生成 + 状态栏反馈（面板入口的呈现由 FloatingMenu 自理）。 */
  async function generateGraphTags() {
    if (!pathRef.current || graphTagsBusy) {
      return;
    }
    setGraphTagsBusy(true);
    onStatus(t("graphTags.generating"));
    const outcome = await runGraphTags();
    setGraphTagsBusy(false);
    const message = outcome.ok ? t("graphTags.done", { count: outcome.tags.length }) : t("error");
    onStatus(message);
  }

  useEffect(() => {
    if (!selectionState) {
      // 只收气泡条。**面板不因选区塌陷而关闭**：它是用户显式召唤出来的，只该由
      // 显式动作（关闭 / Esc / 落地 / 丢弃）收走。此前这里会连面板一起关——而编辑器
      // 一失去焦点（在指令框里打字、点面板上的按钮）浏览器就把选区收成光标，CM 同步
      // 进 state 即触发这一条，于是"打着指令面板自己没了""结果还没落地就不见了"
      // （真机 2026-09-04）。冻结范围由 `ai-session.ts` 的 StateField 跟着文档走，
      // 选区看不见了也不影响「替换」对得准。
      setBubble(null);
      return;
    }
    if (aiSession || pointerDown || mode === "reading") {
      return;
    }
    const timer = window.setTimeout(() => {
      const view = viewRef.current;
      const section = sectionRef.current;
      if (!view || !section) {
        return;
      }
      const box = bubbleBoxFor(view, section, selectionState.from, selectionState.to);
      setBubble(box ? { from: selectionState.from, to: selectionState.to, box } : null);
    }, 200);
    return () => {
      window.clearTimeout(timer);
    };
  }, [selectionState, pointerDown, aiSession, mode]);

  // 拖进正文（B4）：系统文件与文件树条目按落点派给这个正文，这里插成附件 / 笔记链接。
  useEffect(() => {
    const host = hostRef.current;
    if (!host) {
      return;
    }
    const onDrop = (event: Event) => {
      const view = viewRef.current;
      if (view) void dropAt(view, (event as CustomEvent<EditorFileDrop>).detail, pasteContext());
    };
    host.addEventListener(EDITOR_FILE_DROP_EVENT, onDrop);
    return () => {
      host.removeEventListener(EDITOR_FILE_DROP_EVENT, onDrop);
    };
  });

  // 拖选防抖：按住鼠标期间不弹面板，松开后再判定选区稳定。
  useEffect(() => {
    if (isFolder) {
      return;
    }
    const host = hostRef.current;
    if (!host) {
      return;
    }
    const onDown = () => {
      setPointerDown(true);
    };
    const onUp = () => {
      setPointerDown(false);
    };
    host.addEventListener("mousedown", onDown);
    window.addEventListener("mouseup", onUp);
    return () => {
      host.removeEventListener("mousedown", onDown);
      window.removeEventListener("mouseup", onUp);
    };
  }, [isFolder, path]);

  // 编辑器滚动时重算包围盒，气泡与面板跟随选区（超出可视区则贴边）。
  useEffect(() => {
    const view = viewRef.current;
    const anchored = (aiSession && aiSession.scope !== "doc") || bubble !== null;
    if (!view || !anchored) {
      return;
    }
    const scroller = view.scrollDOM;
    const onScroll = () => {
      const section = sectionRef.current;
      if (!section) {
        return;
      }
      setAiSession((current) => {
        if (!current || current.scope === "doc") {
          return current;
        }
        const range = aiRangeOf(view.state) ?? { from: current.from, to: current.to };
        return { ...current, box: selectionBoxFor(view, section, range.from, range.to) };
      });
      setBubble((current) => {
        if (!current) {
          return current;
        }
        const box = bubbleBoxFor(view, section, current.from, current.to);
        return box ? { ...current, box } : current;
      });
    };
    scroller.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      scroller.removeEventListener("scroll", onScroll);
    };
  }, [aiSession?.scope, bubble !== null]);

  async function loadEditorSettings() {
    const result = await commands.listSettings();
    if (result.status !== "ok") {
      return;
    }

    const enabled = result.data.find(
      (spec) => spec.key === "editor.autosave_enabled",
    )?.current_value;
    if (enabled?.kind === "bool") {
      setAutosaveEnabled(enabled.value);
    }

    const debounce = result.data.find(
      (spec) => spec.key === "editor.autosave_debounce_ms",
    )?.current_value;
    if (debounce?.kind === "int" && debounce.value > 0) {
      setAutosaveDebounceMs(debounce.value);
    }

    const floatingContext = result.data.find(
      (spec) => spec.key === "editor.floating_context",
    )?.current_value;
    if (floatingContext?.kind === "bool") {
      setContextEnabled(floatingContext.value);
    }

    const localize = result.data.find(
      (spec) => spec.key === "editor.localize_remote_images",
    )?.current_value;
    if (localize?.kind === "bool") {
      localizeRemoteImagesRef.current = localize.value;
    }

    // 写作视图模式（PRD-06-014，Typora 对标）：设置为唯一事实源，
    // settings://changed 驱动本函数重读 → 所有 pane 同步。
    const typewriter = result.data.find(
      (spec) => spec.key === "editor.typewriter_mode",
    )?.current_value;
    if (typewriter?.kind === "bool") {
      setTypewriterMode(typewriter.value);
    }
    const spelling = result.data.find((spec) => spec.key === "editor.spellcheck")?.current_value;
    if (spelling?.kind === "bool") {
      setSpellcheck(spelling.value);
    }
    const focus = result.data.find((spec) => spec.key === "editor.focus_mode")?.current_value;
    if (focus?.kind === "bool") {
      setFocusMode(focus.value);
    }
    const toolbar = result.data.find(
      (spec) => spec.key === "editor.show_format_toolbar",
    )?.current_value;
    if (toolbar?.kind === "bool") {
      setShowFormatToolbar(toolbar.value);
    }

    // 朗读偏好（TTS，TDD-05 §9）：语速 + 逐句高亮开关。
    const rate = result.data.find((spec) => spec.key === "speech.rate")?.current_value;
    if (rate?.kind === "int" && rate.value > 0) {
      setSpeechRatePct(rate.value);
    }
    const highlight = result.data.find((spec) => spec.key === "speech.highlight")?.current_value;
    if (highlight?.kind === "bool") {
      setSpeechHighlight(highlight.value);
    }
  }

  // ── 文本朗读（TTS，TDD-05 §9）───────────────────────────────────────────────
  // 本编辑器的朗读是否进行中（会话归属本编辑器且非空闲）。
  const readActive =
    readSessionId !== 0 && ttsState.session_id === readSessionId && ttsState.status !== "idle";

  useEffect(() => {
    readSessionRef.current = readSessionId;
  }, [readSessionId]);

  // 切换笔记：停掉本编辑器仍在播的朗读（避免旧文位置误高亮到新文），并复位认领。
  useEffect(() => {
    if (readSessionRef.current !== 0) {
      void ttsStop();
      setReadSessionId(0);
    }
  }, [path]);

  // 逐句高亮 + 跟随滚动（仅 CM 模式可见；阅读态 CM 隐藏，跟读由控制条兜底）。
  useEffect(() => {
    const view = viewRef.current;
    if (!view) {
      return;
    }
    const mine = readSessionId !== 0 && ttsState.session_id === readSessionId;
    const playing = ttsState.status === "speaking" || ttsState.status === "paused";
    if (!mine || !playing) {
      view.dispatch({ effects: setTtsReadingRange.of(null) });
      // 我方会话被抢占/结束：复位认领，使控制条隐藏、切笔记不误停他人朗读。
      if (readSessionId !== 0 && ttsState.session_id !== readSessionId) {
        setReadSessionId(0);
      }
      return;
    }
    if (!speechHighlight) {
      view.dispatch({ effects: setTtsReadingRange.of(null) });
      return;
    }
    const from = readBaseRef.current + ttsState.range_from;
    const to = readBaseRef.current + ttsState.range_to;
    view.dispatch({
      effects:
        ttsState.status === "speaking"
          ? [setTtsReadingRange.of({ from, to }), EditorView.scrollIntoView(from, { y: "center" })]
          : [setTtsReadingRange.of({ from, to })],
    });
  }, [
    readSessionId,
    speechHighlight,
    ttsState.session_id,
    ttsState.status,
    ttsState.index,
    ttsState.range_from,
    ttsState.range_to,
  ]);

  async function startRead(text: string, base: number, markdownMode: boolean) {
    if (text.trim().length === 0) {
      return;
    }
    readBaseRef.current = base;
    const next = await ttsSpeak({
      text,
      markdown: markdownMode,
      scope: `editor:${pathRef.current ?? path ?? ""}`,
    });
    setReadSessionId(next.session_id);
  }

  function toggleReadDocument() {
    if (readActive) {
      void ttsStop();
      return;
    }
    const view = viewRef.current;
    if (!view) {
      return;
    }
    void startRead(view.state.doc.toString(), 0, true);
  }

  // 「存为引文」（ADR-162 ⑭）：选区与它起始所在的行交给 Rust，卡片落进所属项目的研究笔记。
  function saveQuote() {
    const view = viewRef.current;
    const notePath = pathRef.current;
    if (!view || !notePath || !selectionState) return;
    const startLine = view.state.doc.lineAt(selectionState.from).number;
    void commands.creatorSaveQuote(notePath, startLine, selectionState.text).then((result) => {
      onStatus(
        result.status === "ok"
          ? t("quote.saved", { note: noteStem(result.data.research_path) })
          : vaultErrorText(result.error),
      );
    });
  }

  function readSelectionAloud() {
    const view = viewRef.current;
    if (!view || !selectionState) {
      return;
    }
    void startRead(selectionState.text, selectionState.from, false);
  }

  function toggleReadPauseResume() {
    if (ttsState.status === "speaking") {
      void ttsPause();
    } else {
      void ttsResume();
    }
  }

  function changeReadRate(pct: number) {
    setSpeechRatePct(pct);
    void commands.setSetting("speech.rate", { kind: "int", value: pct });
    void commands.ttsSetRate(pct);
  }

  /**
   * Live Preview 装饰组（就地渲染 + 可点交互）：仅 live 模式启用；源码模式呈现
   * 原始 Markdown（语法高亮保留），事实源不变（PRD-06-010/011/017）。
   */
  function buildPresentationExtensions(): Extension {
    const wikiCtx = {
      notePath: () => pathRef.current,
      resolve: resolveWikiTargets,
      onActivate: activateWikiLink,
    };
    return [
      livePreviewExtension(() => pathRef.current, {
        view: t("image.view"),
        resize: t("image.resize"),
        lightbox: { dialog: t("image.lightbox"), close: t("image.close"), previous: t("image.previous"), next: t("image.next") },
      }),
      // 表格块就地渲染（选区触及即还原源码；PRD-06-011 P1）。
      tablePreviewExtension({ cell: (row, column) => t("tableEditing.cell", { row, column }),
        rowHandle: (row) => t("tableEditing.rowHandle", { row }),
        columnHandle: (column) => t("tableEditing.columnHandle", { column }),
        appendRow: t("tableEditing.appendRow"), appendColumn: t("tableEditing.appendColumn"),
        openMenu: (event, target) => openTableMenu(event, target),
        copyRange: (markdown) => {
          const notePath = pathRef.current;
          if (!notePath) return;
          void copyTableAs(markdown, "rich", notePath).then((ok) => {
            if (!ok) onStatus(t("error"));
          });
        },
        pasteCell: (view, input, model, row, column, text, html) => pasteTableCell(view, input, model, row, column, text, html, pasteContext()) }),
      // 公式（`$…$` / `$$…$$`）与 Mermaid 图表：光标不在里面时显示渲染结果（对标方案 D1 / D2）。
      richPreviewExtension({
        diagramError: (message) => tMenu("common:markdown.diagramError", { message }),
        // 嵌入 `![[…]]`（D5）：独占一行的在光标外换成渲染结果，打开与跳转同点击链接一个动作。
        embed: {
          notePath: () => pathRef.current,
          onOpenPath: (target) => onOpenPath?.(target),
          onActivateWikiLink: activateWikiLink,
        },
      }),
      taskCheckboxExtension(),
      inlineTagExtension((tag) => {
        onOpenTag?.(tag);
      }),
      wikiLinkExtension(wikiCtx),
      wikiLinkClickHandler(wikiCtx),
      // 标准 md 链接（自动生成的「来源」区逐条如此）：与阅读面同一条解析命令。
      mdLinkClickHandler({
        notePath: () => pathRef.current,
        onOpenPath: (relPath) => onOpenPath?.(relPath),
      }),
      // 机器脚手架注释（转写块哨兵、对话笔记的出处标记）在预览面让位：它们是 Rust 的定界符，
      // 不是任何人写下的内容。与 frontmatter 折叠同处本函数 = 同样只作用于实时预览。
      lunaScaffoldHidingExtension(),
      frontmatterFoldingExtension({
        label: (count) =>
          count > 0 ? t("frontmatter.labelCount", { count }) : t("frontmatter.label"),
        expandTitle: t("frontmatter.expandTitle"),
        collapseTitle: t("frontmatter.collapseTitle"),
        expandAction: t("frontmatter.expandAction"),
        collapseAction: t("frontmatter.collapseAction"),
      }, {
        // 属性面板里认不出的写法：切到源码模式、光标落到那一行（ADR-139 ①）。
        onEditSource: (view, line) => {
          changeModeRef.current("source");
          const at = view.state.doc.line(Math.min(line, view.state.doc.lines)).from;
          view.dispatch({ selection: { anchor: at }, scrollIntoView: true });
          view.focus();
        },
      }),
    ];
  }

  function buildLocaleExtensions(): Extension {
    return [
      EditorState.phrases.of({
        Find: t("search.find"),
        Replace: t("search.replace"),
        next: t("search.next"),
        previous: t("search.previous"),
        all: t("search.all"),
        "match case": t("search.matchCase"),
        regexp: t("search.regexp"),
        "by word": t("search.byWord"),
        replace: t("search.replaceOne"),
        "replace all": t("search.replaceAll"),
        close: t("search.close"),
        "current match": t("search.currentMatch"),
        "on line": t("search.onLine"),
        "Go to line": t("search.goToLine"),
        go: t("search.go"),
        "replaced $ matches": t("search.replacedMatches"),
        "replaced match on line $": t("search.replacedOnLine"),
      }),
      placeholder(t(`canvas.${canvasDestination}.placeholder`)),
    ];
  }

  useEffect(() => {
    if (!hostRef.current) {
      return;
    }

    const view = new EditorView({
      parent: hostRef.current,
      state: EditorState.create({
        doc: "",
        extensions: [
          // 撤销历史住在 Compartment 里：切换笔记时整个重建（`resetEditorHistory`）。
          historyCompartmentRef.current.of(history()),
          loadGateCompartmentRef.current.of(EditorState.readOnly.of(false)),
          // GFM 基线方言（表格/任务列表/删除线/自动链接，PRD-06-016）；
          // 围栏代码块按 CodeInfo 惰性加载语言解析器（Typora 对标，PRD-06-011）。
          // `addKeymap: false`：语言包自带的 Enter / Backspace 键位挂在 Prec.high，压过
          // 本仓库的全部绑定，且它的退格会把列表标记换成等宽空格、把任务项变成缩进代码块
          // （真机 2026-09-04）。列表 / 引用块的键位由 list-editing.ts + editor-extensions.ts
          // 单一来源承担。
          markdown({
            base: markdownLanguage,
            // 与阅读视图、大纲同一方言（`lib/markdown-dialect.ts`：CJK 友好强调）。
            extensions: EDITOR_MARKDOWN_EXTENSIONS,
            codeLanguages: codeLanguageForFence,
            addKeymap: false,
          }),
          // 括号/引号智能配对（Typora 对标；退格删对、选区包裹）。
          closeBrackets(),
          // markdown 语言级配对集（补反引号；引号在字词后不配对为 CM 既定防误配策略）。
          markdownCloseBracketsData(),
          search({ top: false }),
          // 查找文案与正文提示随界面语言热重配，不重建正文或撤销栈。
          localeCompartmentRef.current.of(buildLocaleExtensions()),
          // 打字机模式插槽（设置驱动热切换，PRD-06-014）。
          typewriterCompartmentRef.current.of([]),
          // 西文拼写检查插槽（设置驱动热切换，W6a）。
          spellcheckCompartmentRef.current.of([]),
          // 使用浏览器原生选区/光标：代码行有实体底色时仍可稳定拖选、复制与输入。
          highlightActiveLine(),
          // CM 自绘光标与选区（`drawSelection`）：原生光标贴着块级部件（折叠的 frontmatter
          // 胶囊）时，WebKit 会画一根与部件同高的粗光标（真机 2026-09-02，颜色修对了形状还在）；
          // CM 按文本坐标画的光标永远是一根细线。颜色归 `styles/integrations/codemirror.css`。
          drawSelection(),
          // 选区矩形改由自绘层画（左界夹到正文列、换行残块不画）；CM 自带的选区层由
          // `integrations/codemirror.css` 藏起来，理由见 selection-layer.ts 抬头。
          selectionLayerExtension(),
          // 浮层（补全、斜杠菜单、表格尺寸选择器）的可用空间按正文可见区算，而不是整个窗口：
          // 否则光标贴近底部时 CM 以为下方还放得下，浮层落到状态栏后面被裁掉（真机 2026-10-04
          // 两条 `[[` 补全只露出一半）。可见区放不下就翻到光标上方。
          tooltips({ tooltipSpace: (view) => view.scrollDOM.getBoundingClientRect() }),
          // 多光标（B5）：⌥ 点击加光标、⌥ 拖动列选；只看主光标的自定义按键在多光标下放行。
          multiCursorExtension(),
          // AI 会话：冻结范围随变更映射 + 落地渐隐高亮；键位（⌘J / Esc / ⌘↩）置于
          // 其它 Esc 绑定之前，无会话时放行给它们。
          aiSessionExtension(),
          aiSessionKeymap({
            escape: () => aiKeyHandlersRef.current.escape(),
            accept: () => aiKeyHandlersRef.current.accept(),
          }),
          // 围栏的自动闭合与空行退出；置于列表续写/默认 Enter 之前。
          codeFenceEditingKeymap(),
          // 表格内 Tab/Shift-Tab 跳单元格（先于缩进 keymap 消费；表外落回缩进）。
          tableEditingKeymap(),
          tablePickerExtension({
            title: t("tableEditing.picker.title"),
            size: (columns, rows) => t("tableEditing.picker.size", { columns, rows }),
          }),
          // 列表编辑手感：项首退格 / Shift+Enter 软换行 / 按列表结构的 Tab、Shift-Tab；
          // 表外、列表外一律放行给下面的通用缩进与默认退格。
          listEditingKeymap(),
          editorProductivityExtensions(),
          // ⌥↑↓ 先按块移动（列表项连子项、表格、引用、代码块围栏），不归它管再逐行移动。
          blockMoveKeymap(),
          // 标题按章节、列表项连子项折叠（B3）：悬停出三角，⌘⌥[ / ⌘⌥]。
          headingFoldExtension({ fold: t("fold.fold"), unfold: t("fold.unfold") }),
          presentationCompartmentRef.current.of(
            modeRef.current === "live" ? buildPresentationExtensions() : [],
          ),
          cueLineHighlightExtension(),
          // 时间戳行的就地起播（PRD-04-066）：装订线播放键 + ⌘/Ctrl 点击；普通单击仍是放
          // 光标——转写正文可编辑，裸单击改成 seek 会让"点进去改个错别字"变成"音频跳走"。
          // 装在隔间里：这一篇没有时间戳行时整个摘掉，不留一条空装订线（见 ref 处注释）。
          timecodeLinesExtension(),
          timecodeCompartmentRef.current.of([]),
          // 全文朗读的「当前句」高亮（TTS，TDD-05 §9）；由 setTtsReadingRange 效果驱动。
          ttsReadingHighlight(),
          completionsExtension({
            notePath: () => pathRef.current,
            slash: {
              label: (key) => t(key),
              run: (_view, command) => {
                if (command.action.kind === "ai") openAiComposerFromMenu();
                else runFormatAction(command.action);
              },
              hidden: (command) =>
                (command.action.kind === "ai" || command.action.kind === "template") && restricted,
            },
          }),
          // 粘贴图片/附件 → 存 assets/ 回填相对链接；远程图片链接 → 下载本地化；选区 + URL → 包成链接（PRD-06-012）。
          pasteAssetsExtension(pasteContext()),
          lunaSyntaxHighlighting(),
          // 代码块整块底色容器（Typora 对标；Live/源码两模式共用）。
          codeBlockLinesExtension(),
          // 列表行悬挂缩进（同为两模式共用：源码里标记可见，宽度同样成立）。
          listIndentExtension(),
          // 文首 frontmatter 按 YAML 原文显示（源码模式；实时预览里整段是胶囊 / 属性面板）。
          frontmatterLinesExtension(),
          // 块间距（同为两模式共用：间距不替换任何标记，源码模式照样是长文）。
          blockSpacingExtension(),
          // 未解析引用式链接去链接色（同为两模式共用：`[名字]` 是不是链接取决于文档里
          // 有没有对应定义，与用哪个视图看无关）。
          unresolvedLinkExtension(),
          EditorView.lineWrapping,
          EditorView.domEventHandlers({
            blur: () => {
              void flushRef.current();
            },
          }),
          EditorView.updateListener.of((update) => {
            if (update.docChanged) {
              if (draftEditsRef.current) draftEditsRef.current = draftEditsRef.current.compose(update.changes);
              // 脏判定不把整篇转成字符串：长度 / 行数不同即脏，相同才逐块比较（`Text.eq`）。
              markDirty(!update.state.doc.eq(savedTextOf(savedContentRef.current)));
              setDocVersion((version) => version + 1);
            }
            // 字数与大纲要扫全文：每次按键都做，长文档的输入延迟就随篇幅涨（对标方案 A5）。
            // 停手一拍再算，期间的改动合成一次。
            if (update.docChanged || syntaxTree(update.state) !== syntaxTree(update.startState)) {
              scheduleDocumentFacts(update.view);
            }
            if (update.selectionSet || update.docChanged) {
              updateSelectionState(update.view);
            }
          }),
          // 键位一律从命令注册表派生（TDD-07 §4.2）：右键菜单与格式工具栏的提示取
          // 同一张表，于是"提示写着 ⌘K、按下去没反应"这类漂移没有发生的空间。
          keymap.of([
            {
              key: codeMirrorKey(editorShortcut("bold")),
              run: toggleBold,
            },
            {
              key: codeMirrorKey(editorShortcut("italic")),
              run: toggleItalic,
            },
            {
              key: codeMirrorKey(editorShortcut("insert_link")),
              run: (viewInstance) => insertLink(viewInstance, t("linkPlaceholder")),
            },
            // Typora 风格段落快捷键：⌘0 正文、⌘1–⌘6 标题（PRD-06-014）。
            ...([0, 1, 2, 3, 4, 5, 6] as const).map((level) => ({
              key: codeMirrorKey(editorShortcut(`heading_${level}`)),
              run: (viewInstance: EditorView) => setHeading(viewInstance, level as HeadingLevel),
            })),
            ...closeBracketsKeymap,
            ...searchKeymap,
            ...defaultKeymap,
            ...historyKeymap,
          ]),
        ],
      }),
    });

    viewRef.current = view;

    // 「Tab 键移动焦点」会话级一份（`lib/tab-focus-mode.ts`）：本编辑器跟着它走。只在变化时
    // 设置——默认关时不碰，CM 自带的「Esc 后 2 秒内 Tab」临时出路照常可用。
    if (tabFocusModeEnabled()) view.setTabFocusMode(true);
    const offTabFocus = subscribeTabFocusMode(() => view.setTabFocusMode(tabFocusModeEnabled()));

    return () => {
      offTabFocus();
      void flushRef.current();
      view.destroy();
      viewRef.current = null;
    };
  }, []);

  // 模式热切换：Live Preview 装饰组经 Compartment reconfigure，不重建实例、不丢
  // 光标与撤销栈（阅读模式下编辑面隐藏，装饰组同源码模式置空即可）。
  //
  // **界面语言也必须重配**：`buildPresentationExtensions()` 闭包捕获了 `t`，而
  // frontmatter 折叠部件的文案是在构造那一刻**取值后交给扩展**的（不是每帧现取）。
  // 只依赖 `mode` 的表现是：切换界面语言后，整屏都换了语言，唯独编辑器里那枚
  // 「元数据 (YAML)」部件停在旧语言，直到用户碰巧切一次视图模式才跟上（真机可见）。
  // 依赖取 `i18n.language` 而不是 `t`：前者是精确的"语言变了"，后者的函数标识在
  // 别的原因下也会变，会招来无谓的重配。
  useEffect(() => {
    const view = viewRef.current;
    if (!view) {
      return;
    }
    view.dispatch({
      effects: presentationCompartmentRef.current.reconfigure(
        mode === "live" ? buildPresentationExtensions() : [],
      ),
    });
  }, [mode, i18n.language]);

  useEffect(() => {
    viewRef.current?.dispatch({ effects: localeCompartmentRef.current.reconfigure(buildLocaleExtensions()) });
  }, [i18n.language, canvasDestination]);

  // 打字机模式热切换（PRD-06-014）：设置驱动，所有 pane 经 settings://changed 同步。
  useEffect(() => {
    const view = viewRef.current;
    if (!view) {
      return;
    }
    view.dispatch({
      effects: typewriterCompartmentRef.current.reconfigure(
        typewriterMode ? typewriterScrolling() : [],
      ),
    });
  }, [typewriterMode]);

  useEffect(() => {
    viewRef.current?.dispatch({
      effects: spellcheckCompartmentRef.current.reconfigure(spellcheck ? spellcheckExtension() : []),
    });
  }, [spellcheck]);

  // Cmd/Ctrl+E：编辑 ⇄ 阅读切换（PRD-06-017；键位归命令注册表，落点与作用域归 view-mode.ts）。
  // 监听挂在 `window` 上——键盘事件只沿「焦点元素 → 祖先」冒泡，挂在本编辑器子树上时
  // 阅读模式（只读 React 视图，焦点落 body）根本收不到；作用域改由 `active` 判定。
  useEffect(() => {
    if (!active || (path !== null && !path.endsWith(".md"))) {
      return;
    }
    const onKeyDown = (event: KeyboardEvent) => {
      if (isEditorSaveShortcut(event)) {
        event.preventDefault(); void flushRef.current(); return;
      }
      if (!respondsToEditorShortcuts({ active, path })) {
        return;
      }
      // 整篇动作的键位是可改的设置（⌘E / ⌘J，W3b）：判定与工作区同一套（含双击），命中后交给
      // 命令面板也走的那个处理器——两个入口一个行为。
      const { bindings, doubleTapWindowMs } = editorKeyStateRef.current;
      const id = editorCommandForKey(event, bindings, {
        detector: editorDoubleTapRef.current,
        windowMs: doubleTapWindowMs,
        nowMs: event.timeStamp,
      });
      if (id === null) {
        return;
      }
      event.preventDefault();
      editorCommandRef.current(id);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [active, path]);

  // 命令面板的编辑器作用域命令（PRD-07-041）：shell 只广播 id，活跃编辑器认领执行——与 `⌘E`
  // 同一分工与同一判活跃谓词。处理器经 ref 读**此刻**的状态与回调（typewriter / focus /
  // 工具栏显隐是 state，朗读与版本历史是组件内函数），effect 依赖只有活跃与路径。
  useEffect(() => {
    if (!respondsToEditorShortcuts({ active, path })) {
      return;
    }
    const onCommand = (event: Event) => {
      const id = (event as CustomEvent<EditorCommandDetail>).detail?.id;
      if (id) {
        editorCommandRef.current(id);
      }
    };
    window.addEventListener(EDITOR_COMMAND_EVENT, onCommand);
    return () => {
      window.removeEventListener(EDITOR_COMMAND_EVENT, onCommand);
    };
  }, [active, path]);

  // 编辑器定位（大纲 PRD-06-014 / 搜索命中 PRD-06-031）：面板经 window 事件请求定位，
  // 阅读面的定位等新正文渲染出来再兑现（加载完成处只暂存，见那里的说明）。
  useEffect(() => {
    const pending = revealPendingRef.current;
    if (!pending || mode !== "reading") return;
    if (applyReveal(pending)) revealPendingRef.current = null;
  }, [docVersion, mode, applyReveal]);

  // 按当前路径匹配响应（多 pane 同笔记各自滚动）。Live/源码 = 光标落该行 + 滚动；
  // 阅读 = 按序滚动到渲染后的第 N 个标题（无标题序号则只打开不滚动）。
  // 目标笔记还没加载完时暂存请求，由加载完成处兑现（`revealPendingRef`）。
  useEffect(() => {
    const onReveal = (event: Event) => {
      const detail = (event as CustomEvent<EditorRevealDetail>).detail;
      if (!detail || detail.path !== pathRef.current) {
        return;
      }
      // 就地处理即清掉模块级暂存：防同一条请求在稍后加载/重挂载时迟到重放。
      consumePendingEditorReveal(detail.path);
      if (!applyReveal(detail)) {
        revealPendingRef.current = detail;
      }
    };
    window.addEventListener(EDITOR_REVEAL_EVENT, onReveal);
    return () => {
      window.removeEventListener(EDITOR_REVEAL_EVENT, onReveal);
    };
  }, [applyReveal]);

  // 大纲拖动调段序（PRD-19-011）：搬段在 Rust（与大纲分段同一判定），这里只把结果作为一次普通
  // 编辑落进缓冲区——进撤销栈（⌘Z 撤回这次调整），保存走同一条保存路径。只认活跃编辑器：
  // 同一篇开在两个分栏里时两边都收到请求，各搬一次就乱了。
  useEffect(() => {
    if (!respondsToEditorShortcuts({ active, path })) {
      return;
    }
    const onMove = (event: Event) => {
      const detail = (event as CustomEvent<ScriptSectionMoveDetail>).detail;
      const view = viewRef.current;
      if (!detail || !view || detail.path !== pathRef.current || view.state.readOnly) {
        return;
      }
      const before = view.state.doc.toString();
      void commands.scriptMoveSection(before, detail.fromLine, detail.beforeLine).then((result) => {
        const current = viewRef.current;
        // 等结果的这一会儿里又打了字或换了笔记：这次结果对不上，不落。
        if (!result || !current || pathRef.current !== detail.path || current.state.doc.toString() !== before) {
          return;
        }
        current.dispatch({ changes: result.change, userEvent: "move.section" });
        // 光标跟到被搬那段的标题、滚到可见处，但不抢焦点：在大纲里连按 ⌥↑ / ⌥↓ 不能被打断。
        const heading = current.state.doc.line(Math.max(1, Math.min(result.line, current.state.doc.lines)));
        current.dispatch({ selection: { anchor: heading.from }, effects: EditorView.scrollIntoView(heading.from, { y: "center" }) });
      });
    };
    window.addEventListener(SCRIPT_SECTION_MOVE_EVENT, onMove);
    return () => {
      window.removeEventListener(SCRIPT_SECTION_MOVE_EVENT, onMove);
    };
  }, [active, path]);

  // B-roll 镜头勾选（PRD-19-026）：改那个 `broll` 代码块的围栏行（Rust 判），作为一次普通编辑落进
  // 缓冲区（可撤销），保存走同一条路径；只认活跃编辑器，理由同搬段。不动光标与选区。
  useEffect(() => {
    if (!respondsToEditorShortcuts({ active, path })) {
      return;
    }
    const onToggle = (event: Event) => {
      const detail = (event as CustomEvent<ScriptBrollToggleDetail>).detail;
      const view = viewRef.current;
      if (!detail || !view || detail.path !== pathRef.current || view.state.readOnly) {
        return;
      }
      const before = view.state.doc.toString();
      void commands.scriptSetBrollDone(before, detail.line, detail.done).then((change) => {
        const current = viewRef.current;
        if (!change || !current || pathRef.current !== detail.path || current.state.doc.toString() !== before) {
          return;
        }
        current.dispatch({ changes: change, userEvent: "input.broll" });
      });
    };
    window.addEventListener(SCRIPT_BROLL_TOGGLE_EVENT, onToggle);
    return () => {
      window.removeEventListener(SCRIPT_BROLL_TOGGLE_EVENT, onToggle);
    };
  }, [active, path]);

  // 「插入引文」（ADR-162 ⑭）：嵌入作为一次普通编辑落在光标所在行之后自成一行（可撤销）；
  // 只认活跃编辑器，理由同搬段。
  useEffect(() => {
    if (!respondsToEditorShortcuts({ active, path })) {
      return;
    }
    const onInsert = (event: Event) => {
      const detail = (event as CustomEvent<ScriptInsertDetail>).detail;
      const view = viewRef.current;
      if (!detail || !view || detail.path !== pathRef.current || view.state.readOnly) {
        return;
      }
      // 光标在空行上就占这一行；否则接在这一行之后、空一行另起。
      const line = view.state.doc.lineAt(view.state.selection.main.head);
      const blank = line.text.trim().length === 0;
      const insert = blank ? detail.text : `\n\n${detail.text}`;
      const from = blank ? line.from : line.to;
      const end = from + insert.length;
      view.dispatch({
        changes: { from, to: line.to, insert },
        selection: detail.select ? { anchor: end - detail.text.length, head: end } : { anchor: end },
        effects: EditorView.scrollIntoView(end, { y: "center" }),
        userEvent: "input.quote",
      });
      // 这是用户点了按钮要的插入，当场落盘（与编辑器里的保存同一条串行路径）：真机 2026-10-18——
      // 编辑器不在焦点上时，这次插入一直停在「未保存」，直到再点一下正文才写盘。
      void flushRef.current();
    };
    window.addEventListener(SCRIPT_INSERT_EVENT, onInsert);
    return () => {
      window.removeEventListener(SCRIPT_INSERT_EVENT, onInsert);
    };
  }, [active, path]);

  // 脚本工具栏的「AI 改稿」（ADR-165 ⑦）：有选区改选区；没有就选中光标所在那一段（段界由 Rust 的
  // 分段给出：本段标题行到下一段标题行之前），再打开与选区气泡同一个 AI 面板并直接开跑。
  useEffect(() => {
    if (!respondsToEditorShortcuts({ active, path })) {
      return;
    }
    const onRewrite = (event: Event) => {
      const detail = (event as CustomEvent<ScriptRewriteDetail>).detail;
      const view = viewRef.current;
      if (!detail || !view || detail.path !== pathRef.current || view.state.readOnly) {
        return;
      }
      const main = view.state.selection.main;
      let from = main.from;
      let to = main.to;
      if (from === to) {
        const starts = (liveScriptRef.current?.sections ?? []).map((section) => section.line);
        const range = scriptSectionRangeAt(view.state.doc.toString(), starts, main.head);
        if (!range) return;
        ({ from, to } = range);
      }
      if (from === to) return;
      view.dispatch({ selection: { anchor: from, head: to } });
      openSelectionSession({ from, to, text: view.state.sliceDoc(from, to) }, { autoRun: detail.action });
    };
    window.addEventListener(SCRIPT_REWRITE_EVENT, onRewrite);
    return () => {
      window.removeEventListener(SCRIPT_REWRITE_EVENT, onRewrite);
    };
  });

  // 版本历史的外部入口（归入建议条的「查看变更」，PRD-16-025）。两个消费面：
  // ① 事件监听——目标页**已经开着**时立即弹；② 加载完成处兑现暂存——目标页**刚被
  // 打开**时，事件派发那一刻本组件的 path 还是旧值（React 未重渲染），只能靠暂存接住。
  // 只做 ① 的话，「查看变更」翻一篇没开着的主题页会静默丢失（验收轮缺陷）。
  useEffect(() => {
    const onRequest = (event: Event) => {
      const detail = (event as CustomEvent<VersionHistoryRequestDetail>).detail;
      if (detail?.path === pathRef.current && consumePendingVersionHistory(detail.path)) {
        setVersionHistoryOpen(true);
      }
    };
    window.addEventListener(VERSION_HISTORY_EVENT, onRequest);
    return () => {
      window.removeEventListener(VERSION_HISTORY_EVENT, onRequest);
    };
  }, []);

  useEffect(() => {
    saveActionRef.current = async () => {
      const view = viewRef.current;
      const currentPath = pathRef.current;
      if (!dirtyRef.current) {
        return true;
      }
      // 没装载成功的这一篇不写（只读兜底之外再挡一次）：写进去的只会是别的内容。
      if (loadFailedRef.current) return true;
      if (externalPendingRef.current) return false;

      const content = view?.state.doc.toString() ?? documentContentRef.current;
      // 还没落盘的画布里只有空白（例如键盘用户 Tab 进编辑区敲出的缩进）：没有可存的东西，不去建
      // 一篇只有空白的笔记——Rust 会拒，此前状态栏就挂着一句「错误」（E4 键盘可达真机 2026-10-06）。
      if (!currentPath && content.trim() === "") {
        markDirty(false);
        onStatus("");
        return true;
      }
      onStatus(t("saving"));
      if (!currentPath) {
        draftEditsRef.current = ChangeSet.empty(content.length);
        // 落盘形态**跟随目的地**（PRD-06-058 P0-1）：前端交出去的是"这块画布长在哪一格"
        // 这个事实，落成记录还是知识笔记由 Rust 判定——前端不在这里 if/else 挑命令挑类型。
        const createResult = await commands.createNoteFromText(content, canvasDestinationRef.current);
        const edits = draftEditsRef.current;
        draftEditsRef.current = null;
        if (createResult.status === "ok") {
          const saved = createResult.data;
          pathRef.current = saved.path;
          savedContentRef.current = saved.content;
          loadedPathRef.current = saved.path;
          if (edits) {
            const rebased = rebaseCreatedDraft(content, saved.content, edits);
            if (viewRef.current) applyIncrementalText(viewRef.current, rebased);
            documentContentRef.current = rebased;
          }
          setNameDraft(displayNameFromPath(saved.path));
          markDirty(currentDocumentText() !== saved.content);
          onSavedPath?.(saved.path, null);
          onStatus("");
          return true;
        }
        // 说清是哪种失败（磁盘 / 权限 / 库没打开…），不是一句笼统的「错误」。
        onStatus(vaultErrorText(createResult.error));
        return false;
      }

      const savedBeforeWrite = savedContentRef.current;
      savingContentRef.current = content;
      const result = await commands.writeNote(currentPath, content, savedBeforeWrite);
      savingContentRef.current = null;
      if (result.status === "ok") {
        if (result.data.kind === "conflict") {
          setConflictDiff(null);
          externalContentRef.current = result.data.current_content;
          externalPendingRef.current = true;
          setExternalPending(true);
          onStatus(t("externalConflict"));
          return false;
        }
        if (savedContentRef.current === savedBeforeWrite) savedContentRef.current = content;
        // 自动标签等后续 Lunarca 写入可能已通过 `note://written` 更新了基线；绝不能在这里
        // 用较旧的 `content` 覆盖它。用户在请求期间继续键入时也必须维持脏态。
        const currentContent = currentDocumentText();
        const stillDirty = currentContent !== savedContentRef.current;
        markDirty(stillDirty);
        onStatus(result.data.index_updated ? "" : t("savedIndexPending"));
        // 打开同一笔记的其余 pane / 弹出窗口由 Rust `note://written` 事件同步
        // （write_note 内广播，跨窗口有效；接收端见下方 events.noteWritten 监听）。
        return true;
      }

      savedContentRef.current = savedBeforeWrite;
      const currentContent = currentDocumentText();
      markDirty(currentContent !== savedBeforeWrite);
      onStatus(t("error"));
      return false;
    };
  }, [onSavedPath, onStatus, t]);

  useEffect(() => {
    void loadEditorSettings();
    const unlisten = events.settingsChanged.listen((event) => {
      if (event.payload.key.startsWith("editor.")) {
        void loadEditorSettings();
      }
    });

    return () => {
      void unlisten.then((dispose) => {
        dispose();
      });
    };
  }, []);

  // ── Wiki-link：库里增删改名后，之前「指得到 / 指不到」的回答作废，重问 Rust ──────
  /** 点一条 wiki-link：打开 / 跳到片段 / 新建都按 Rust 的回答（`openWikiLink`）。 */
  function activateWikiLink(target: string) {
    const from = pathRef.current;
    if (!from) return;
    void openWikiLink(from, target, {
      onOpenPath: (relPath) => onOpenPath?.(relPath),
      allowCreate: true,
      onError: () => onStatus(t("error")),
    });
  }

  // ⌘ 悬停链接预览（C1）：编辑面的链接从文档里取目标，阅读面的从 data 属性取。
  const linkPreview = useLinkPreview(sectionRef, isFolder ? null : path, (element): LinkRef | null => {
    const view = viewRef.current;
    if (view?.contentDOM.contains(element)) {
      const wiki = wikiTargetAtElement(view, element);
      if (wiki !== null) return { kind: "wiki", target: wiki };
      const href = mdLinkTargetAtElement(view, element);
      return href && !isSafeExternalUrl(href) ? { kind: "md", target: href } : null;
    }
    return linkRefFromDataset(element);
  });

  const resetWikiLinks = useCallback(() => {
    viewRef.current?.dispatch({ effects: resetWikiResolutions.of(null) });
  }, []);

  // 合并同一阵变更的实现只有一份（`use-vault-reload`）：一次批量移动不会被放大成 N 次重问。
  useVaultReload(resetWikiLinks);

  useEffect(() => {
    if (!autosaveEnabled || !dirty || externalPending || autosaveDebounceMs === null) {
      return;
    }

    const timer = window.setTimeout(() => {
      void flushRef.current();
    }, autosaveDebounceMs);

    return () => {
      window.clearTimeout(timer);
    };
  }, [autosaveDebounceMs, autosaveEnabled, dirty, docVersion, externalPending]);

  useEffect(() => {
    let active = true;

    async function loadSelectedPath() {
      const flushed = await flushRef.current();
      if (!active || !flushed) {
        return;
      }
      if (path && loadedPathRef.current === path && pathRef.current === path) return;
      // 离开的这一篇已经落盘（上面的 flush）：记下它的历史与光标，切回来正文没变就接上。
      const leaving = loadedPathRef.current;
      const leavingState = viewRef.current?.state;
      if (leaving && leavingState) {
        const saved = saveEditorHistory(leavingState);
        if (saved) historyCacheRef.current.put(leaving, saved);
      }
      externalContentRef.current = null;
      externalPendingRef.current = false;
      // 装载完成前大纲是「还没算」，不是「没有标题」：右侧显示加载中，而不是闪一下空状态。
      setLiveOutline(undefined);
      noteTypeRef.current = null;
      setNoteType(null);
      refreshScriptOutline("");
      setExternalPending(false);
      // path=null（空标签页/取消选择）必须真正清空编辑面——残留上一篇会把后续
      // 输入写进旧笔记（多标签下每个空标签都是独立编辑起点，PRD-07-035）。
      pathRef.current = path;
      setNameDraft(displayNameFromPath(path));
      setRenameError(null);

      if (!path || !path.endsWith(".md")) {
        setLoadError(null);
        savedContentRef.current = "";
        loadedPathRef.current = null;
        setWordCount(0);
        markDirty(false);
        setSelectionState(null);
        closeAiSession();
        if (viewRef.current) {
          applyIncrementalText(viewRef.current, "");
          resetEditorHistory(viewRef.current, historyCompartmentRef.current);
        }
        return;
      }
      onStatus(t("loading"));
      const result = await commands.readNote(path);
      if (!active) {
        return;
      }

      if (result.status !== "ok") {
        // 按码说清是哪种打不开（例如「还在 iCloud 上，已开始下载」，TDD-06 §4.1 ②）；
        // 一句笼统的「错误」让用户无从下手。
        const message = vaultErrorText(result.error);
        onStatus(message);
        setLoadError(message);
        loadFailedRef.current = true;
        savedContentRef.current = "";
        loadedPathRef.current = null;
        markDirty(false);
        setWordCount(0);
        setWordGoal(null);
        const failedView = viewRef.current;
        if (failedView) {
          applyIncrementalText(failedView, "");
          resetEditorHistory(failedView, historyCompartmentRef.current);
          failedView.dispatch({
            effects: loadGateCompartmentRef.current.reconfigure(EditorState.readOnly.of(true)),
          });
        }
        return;
      }
      setLoadError(null);
      if (loadFailedRef.current) {
        loadFailedRef.current = false;
        viewRef.current?.dispatch({
          effects: loadGateCompartmentRef.current.reconfigure(EditorState.readOnly.of(false)),
        });
      }

      const view = viewRef.current;
      if (!view) {
        return;
      }

      savedContentRef.current = result.data.content;
      loadedPathRef.current = path;
      applyIncrementalText(view, result.data.content);
      // 换了一篇：上一篇的撤销历史不能跟过来（装载本身已不进历史，见 applyIncrementalText）；
      // 这一篇先前编辑过、正文又没被外部改过，就接上它自己的历史与光标。
      const restored = historyCacheRef.current.take(path, view.state.doc);
      resetEditorHistory(view, historyCompartmentRef.current, restored ?? undefined);
      // 光标落到正文首行而不是文档位置 0：位置 0 紧贴折叠的 frontmatter 块级部件，WebKit 会在
      // 那里画一根与部件同高的原生粗光标（真机 2026-09-02，深色主题下尤其刺眼）；且用户
      // 打开一篇笔记要写的是正文，不是元数据。
      const frontmatter = getFrontmatterRange(view.state);
      view.dispatch({
        selection: restored
          ? { anchor: restored.anchor, head: restored.head }
          : { anchor: frontmatter ? Math.min(frontmatter.to + 1, view.state.doc.length) : 0 },
        scrollIntoView: restored !== null,
        effects: [
          // 同名笔记按所在目录消歧：换了一篇，上一篇得到的回答不再成立。
          resetWikiResolutions.of(null),
          // 离开时折着的章节，回来照样折着（正文没变才有 `restored`）。
          ...(restored?.folds.map((range) => foldEffect.of(range)) ?? []),
        ],
      });
      setWordCount(noteWordCount(result.data.content));
      setWordGoal(result.data.meta.word_goal);
      // 大纲同字数一样在换篇时同步算一次：只靠去抖那一拍，右侧大纲会先显示上一篇的标题，
      // 此时点下去跳到错的位置（真机 2026-10-04）。
      setLiveOutline(outlineFromState(view.state, frontmatter?.to ?? 0));
      noteTypeRef.current = result.data.meta.note_type;
      setNoteType(result.data.meta.note_type);
      refreshScriptOutline(result.data.content);
      setSelectionState(null);
      closeAiSession();
      markDirty(false);
      onStatus("");
      openPendingSessionIfAny(path);

      // 「先打开这篇、再跳到那一行」：搜索命中点开的笔记此刻才有内容，兑现暂存的定位。
      // 两层暂存各盖一幕：实例级 = 事件到达时**路径已匹配**但内容未加载；模块级 =
      // 事件到达时**这篇还没打开**（监听器按旧 pathRef 直接丢弃，只有暂存能接住）。
      let reveal: EditorRevealDetail | null = null;
      if (revealPendingRef.current?.path === path) {
        reveal = revealPendingRef.current;
        revealPendingRef.current = null;
      } else if (path) {
        reveal = consumePendingEditorReveal(path);
      }
      if (reveal && modeRef.current === "reading") {
        // 阅读面此刻还是上一篇的 DOM（新正文要等 React 下一次渲染）：按序找第 N 个标题会
        // 找不到或找错。暂存起来，由渲染后的 effect 兑现（真机 2026-10-04：阅读模式点
        // `[[笔记#标题]]` 打开了却停在顶部）。
        revealPendingRef.current = reveal;
        setDocVersion((version) => version + 1);
      } else if (reveal) {
        applyReveal(reveal);
      }
      // 版本历史请求的兑现（同上：目标页刚被「查看变更」打开，事件早已错过本组件）。
      if (consumePendingVersionHistory(path)) {
        setVersionHistoryOpen(true);
      }
    }

    void loadSelectedPath();
    return () => {
      active = false;
    };
  }, [path, onStatus, applyReveal, loadAttempt]);

  useEffect(() => {
    const unlisten = events.vaultChanged.listen((event) => {
      const currentPath = pathRef.current;
      if (currentPath && event.payload.rel_path === currentPath && event.payload.deleted && dirtyRef.current) {
        setConflictDiff(null);
        externalContentRef.current = null;
        externalPendingRef.current = true;
        setExternalPending(true);
        onStatus(t("externalDeleted"));
        return;
      }
      if (!currentPath || event.payload.rel_path !== currentPath || dirtyRef.current) {
        if (
          currentPath &&
          event.payload.rel_path === currentPath &&
          dirtyRef.current &&
          !event.payload.deleted
        ) {
          void commands.readNote(currentPath).then((result) => {
            if (result.status !== "ok" || !dirtyRef.current || pathRef.current !== currentPath ||
                result.data.content === savedContentRef.current || result.data.content === savingContentRef.current) {
              return;
            }
            externalContentRef.current = result.data.content;
            setConflictDiff(null);
            externalPendingRef.current = true;
            setExternalPending(true);
            onStatus(t("externalConflict"));
          });
        }
        return;
      }

      void commands.readNote(currentPath).then((result) => {
        const view = viewRef.current;
        if (result.status !== "ok" || !view || dirtyRef.current || pathRef.current !== currentPath) {
          return;
        }
        // 外部改了这一篇（含另一扇窗设了字数目标）：目标跟着磁盘上的 `luna:` 块走。
        setWordGoal(result.data.meta.word_goal);

        // 先前没装载成功的这一篇出现了（例如从 iCloud 下载回来，TDD-06 §4.1 ②）：解除只读、
        // 认它为已装载，随后照常显示正文。
        if (loadFailedRef.current) {
          loadFailedRef.current = false;
          setLoadError(null);
          loadedPathRef.current = currentPath;
          view.dispatch({
            effects: loadGateCompartmentRef.current.reconfigure(EditorState.readOnly.of(false)),
          });
          setWordCount(noteWordCount(result.data.content));
        }
        savedContentRef.current = result.data.content;
        if (applyIncrementalText(view, result.data.content)) {
          setWordCount(noteWordCount(result.data.content));
          onStatus(t("externalReload"));
        }
      });
    });

    return () => {
      void unlisten.then((dispose) => {
        dispose();
      });
    };
  }, [onStatus, t]);

  // 多 pane / 弹出窗口同文件同步（TDD-07 §4.1）：他处写盘后本实例增量跟进；本实例有
  // 未保存改动时转外部冲突横幅（与 vault://changed 外部修改同一交互）。跨窗口经 Rust
  // `note://written` 事件（write_note 广播），同一 webview 内多 pane 亦收到同一事件。
  useEffect(() => {
    const unlisten = events.noteWritten.listen((event) => {
      const { path: writtenPath, content } = event.payload;
      // 库上下文过滤（PRD-06-050）：副库窗口保存的同名路径不得注入主窗标签页，反之亦然。
      if (event.payload.origin !== vaultContext) {
        return;
      }
      const currentPath = pathRef.current;
      const view = viewRef.current;
      if (!view || !currentPath || writtenPath !== currentPath) {
        return;
      }
      // 以 savedContentRef 为准跳过自写：本实例刚保存的正文即使随后用户继续键入，
      // 也不会被误判为外部冲突（比 doc 现值更可靠）。
      if (savedContentRef.current === content || savingContentRef.current === content) {
        if (savingContentRef.current === content) savedContentRef.current = content;
        return;
      }
      if (dirtyRef.current && view.state.doc.toString() !== savedContentRef.current) {
        const showConflict = () => {
          setConflictDiff(null);
          externalContentRef.current = content;
          externalPendingRef.current = true;
          setExternalPending(true);
          onStatus(t("externalConflict"));
        };
        // 只改了元数据（后台补写 AI 标签、项目阶段）：正文没动，用户没有要选的东西——
        // 三路合并并进缓冲区，不打断打字；合并不干净才照旧弹横幅（`metadata-write.ts`）。
        if (!externalPendingRef.current && isMetadataOnlyWrite(savedContentRef.current, content)) {
          const base = savedContentRef.current;
          const local = view.state.doc.toString();
          void commands.noteMerge(base, local, content).then((result) => {
            const current = viewRef.current;
            // 等合并的这一会儿里换了笔记、又保存了、或用户又打了字：这次结果对不上，退回横幅。
            if (
              !current ||
              pathRef.current !== writtenPath ||
              savedContentRef.current !== base ||
              current.state.doc.toString() !== local ||
              result.status !== "ok" ||
              result.data.kind !== "merged"
            ) {
              if (pathRef.current === writtenPath && savedContentRef.current === base) showConflict();
              return;
            }
            // 不进撤销栈：⌘Z 撤的应当是用户自己打的字，而不是后台写回的标签。
            current.dispatch({ changes: result.data.change, annotations: Transaction.addToHistory.of(false) });
            savedContentRef.current = content;
            markDirty(current.state.doc.toString() !== content);
          });
          return;
        }
        showConflict();
        return;
      }
      savedContentRef.current = content;
      if (applyIncrementalText(view, content)) {
        setWordCount(noteWordCount(content));
      }
    });
    return () => {
      void unlisten.then((dispose) => dispose());
    };
  }, [onStatus, t, vaultContext]);

  async function reloadExternalContent() {
    const requestedPath = pathRef.current;
    if (!requestedPath) return;
    const result = await commands.readNote(requestedPath);
    if (result.status !== "ok" || pathRef.current !== requestedPath) { onStatus(t("error")); return; }
    const view = viewRef.current;
    const externalContent = result.data.content;
    if (!view || externalContent === null) {
      return;
    }

    savedContentRef.current = externalContent;
    applyIncrementalText(view, externalContent);
    setWordCount(noteWordCount(externalContent));
    externalContentRef.current = null;
    externalPendingRef.current = false;
    setExternalPending(false);
    markDirty(false);
    onStatus(t("externalReload"));
  }

  function keepLocalContent() {
    if (externalContentRef.current === null) return;
    savedContentRef.current = externalContentRef.current;
    externalContentRef.current = null;
    externalPendingRef.current = false;
    setExternalPending(false);
    // “保留本地”并不必然意味着有尚未保存的字节：例如外部变更与当前编辑器内容
    // 恰好相同。状态与保存按钮必须由同一份内容基线推导，不能留下“未保存但不能保存”。
    const localContent = viewRef.current?.state.doc.toString() ?? "";
    const stillDirty = localContent !== savedContentRef.current;
    markDirty(stillDirty);
    onStatus("");
    void flushRef.current();
  }

  function compareExternalContent() {
    const local = viewRef.current?.state.doc.toString() ?? "";
    const external = externalContentRef.current;
    const requestedPath = pathRef.current;
    void commands.noteEditDiff(external ?? "", local).then((result) => {
      if (result.status !== "ok") { onStatus(t("error")); return; }
      if (externalPendingRef.current && requestedPath === pathRef.current && external === externalContentRef.current && local === currentDocumentText()) setConflictDiff(result.data);
    }).catch(() => onStatus(t("error")));
  }

  /**
   * 「合并两边改动」（ADR-148 ②、TDD-06 §4.2）：基线 = 已保存内容，我方 = 缓冲区，对方 = 磁盘。
   * 能合并时结果作为一次可撤销的编辑落进缓冲区，并以磁盘版本为新的保存基线走同一条保存护栏
   * （与「用本地改动覆盖」同一出口）；两边改了同一处就说清楚并展开对比，不猜。
   */
  async function mergeExternalContent() {
    const view = viewRef.current;
    const requestedPath = pathRef.current;
    const external = externalContentRef.current;
    if (!view || !requestedPath || external === null) return;
    const local = view.state.doc.toString();
    const result = await commands.noteMerge(savedContentRef.current, local, external);
    if (result.status !== "ok") { onStatus(t("error")); return; }
    // 等合并的这一会儿里横幅已处理、换了笔记、外部又改了或用户又打了字：这次结果对不上了，不落。
    if (!externalPendingRef.current || pathRef.current !== requestedPath || externalContentRef.current !== external || currentDocumentText() !== local) return;
    const outcome = result.data;
    if (outcome.kind !== "merged") {
      onStatus(t(outcome.kind === "conflict" ? "conflictMergeOverlap" : "conflictMergeTooLarge"));
      compareExternalContent();
      return;
    }
    view.dispatch({ changes: outcome.change, userEvent: "input.merge" });
    const merged = view.state.doc.toString();
    setWordCount(noteWordCount(merged));
    savedContentRef.current = external;
    externalContentRef.current = null;
    externalPendingRef.current = false;
    setExternalPending(false);
    setConflictDiff(null);
    markDirty(merged !== external);
    onStatus(t("conflictMerged"));
    void flushRef.current();
  }

  async function saveConflictCopy() {
    if (conflictCopyBusyRef.current) return;
    const content = viewRef.current?.state.doc.toString();
    if (content === undefined) return;
    const previousPath = pathRef.current;
    conflictCopyBusyRef.current = true;
    const result = await commands.createNoteFromText(content, "notes").finally(() => { conflictCopyBusyRef.current = false; });
    if (result.status !== "ok") { onStatus(t("error")); return; }
    if (previousPath !== pathRef.current) return;
    pathRef.current = result.data.path;
    loadedPathRef.current = result.data.path;
    savedContentRef.current = result.data.content;
    externalContentRef.current = null;
    externalPendingRef.current = false;
    setExternalPending(false);
    setNameDraft(displayNameFromPath(result.data.path));
    markDirty(viewRef.current?.state.doc.toString() !== result.data.content);
    onSavedPath?.(result.data.path, previousPath);
    onStatus(t("conflictCopySaved"));
  }

  // AI 结果落地（PRD-06-021）：以会话冻结的范围为准（结果生成期间用户可能移动光标），
  // 范围夹回当前文档长度防越界；写入进 CM 撤销栈，可 Cmd+Z 回退。
  /**
   * AI 结果落地（PRD-06-021）。范围取 `ai-session.ts` 里随文档变更映射过的冻结范围
   * （兜底：打开时的快照，夹回文档长度）；改动入 CM 撤销栈；新文渐隐高亮 + 底部横条。
   * 落地形态（替换 / 插入下方 / 顶部 / 文末 / 此处）由 Rust 建议、用户拍板，这里只执行。
   */
  function landAiResult(text: string, landing: FloatingMenuLanding) {
    const view = viewRef.current;
    const session = aiSession;
    if (!view || !session) {
      return;
    }
    if (landing === "discard" || !text) {
      closeAiSession();
      return;
    }
    const docLength = view.state.doc.length;
    const range = aiRangeOf(view.state) ?? {
      from: Math.min(session.from, docLength),
      to: Math.min(session.to, docLength),
    };
    let from = range.to;
    let to = range.to;
    let insert = text;
    switch (landing) {
      case "replace": {
        from = range.from;
        // 选区常带着段尾换行（三击选段、拖到下一行行首）；模型输出没有。原样替换会把
        // 段落与下一个标题之间的空行吃掉，故保留原选区首尾的空白。
        const original = view.state.sliceDoc(from, to);
        const leading = /^\s*/u.exec(original)?.[0] ?? "";
        const trailing = /\s*$/u.exec(original)?.[0] ?? "";
        insert = `${leading}${text.trim()}${trailing}`;
        break;
      }
      case "insert_below":
      case "insert_end":
        if (landing === "insert_end") {
          from = docLength;
          to = docLength;
        }
        insert = `\n\n${text}`;
        break;
      case "insert_top": {
        const frontmatter = getFrontmatterRange(view.state);
        from = frontmatter ? Math.min(frontmatter.to + 1, docLength) : 0;
        to = from;
        insert = `${text}\n\n`;
        break;
      }
      case "insert_cursor": {
        // 光标停在有字的一行：行尾 → 另起段落；行首 → 插完补空行；行中 → 原样接上。
        const line = view.state.doc.lineAt(from);
        if (line.text.trim().length > 0) {
          if (from >= line.to) {
            insert = `\n\n${text}`;
          } else if (from <= line.from) {
            insert = `${text}\n\n`;
          }
        }
        break;
      }
    }
    const landedText = landing === "replace" ? text.trim() : text;
    const textStart = from + insert.indexOf(landedText);
    view.dispatch({
      changes: { from, to, insert },
      selection: { anchor: from + insert.length },
      userEvent: "input",
      effects: flashLandedRange.of({ from: textStart, to: textStart + landedText.length }),
    });
    window.setTimeout(() => {
      if (viewRef.current === view) {
        view.dispatch({ effects: flashLandedRange.of(null) });
      }
    }, LANDED_HIGHLIGHT_MS);
    setLanded({ landing });
    closeAiSession();
    // 保存流程会先写「保存中」再清空状态栏，落地回执要等它写完再说。
    const landedMessage = t(landing === "replace" ? "floating.landed.replace" : "floating.landed.insert");
    void flushRef.current().then(() => {
      onStatus(landedMessage);
    });
  }

  const dismissLanded = useCallback(() => {
    setLanded(null);
  }, []);

  function undoLanded() {
    const view = viewRef.current;
    setLanded(null);
    if (view) {
      undo(view);
      view.focus();
      void flushRef.current();
    }
  }

  /** 编辑器 keymap 经 ref 读**此刻**的会话与面板处理器（keymap 只建一次）。 */
  const aiKeyHandlersRef = useRef<AiSessionKeyHandlers>({
    escape: () => false,
    accept: () => false,
  });
  aiKeyHandlersRef.current = {
    escape: () => {
      if (!aiSession) {
        // 没有会话时 Esc 只收气泡条，选区留着（还可能要复制 / 继续键入）；
        // 没有气泡也没有会话就放行给后面的 Esc 绑定（关查找面板等）。
        if (bubble) {
          setBubble(null);
          return true;
        }
        return false;
      }
      if (aiControlsRef.current) {
        return aiControlsRef.current.escape();
      }
      closeAiSession();
      return true;
    },
    accept: () => (aiSession ? (aiControlsRef.current?.accept() ?? false) : false),
  };

  async function renameCurrentNote() {
    const wantedName = nameDraft.trim();
    if (!wantedName) {
      setRenameError(t("nameEmpty"));
      return;
    }
    const saved = await flushRef.current();
    if (!saved) {
      return;
    }
    const currentPath = pathRef.current;
    if (!currentPath) {
      return;
    }
    if (displayNameFromPath(currentPath) === wantedName.replace(/\.md$/u, "")) {
      setRenameError(null);
      return;
    }
    onStatus(t("renaming"));
    // 笔记 / 资料 / 文件夹同一条命令（`vault_rename`）：后缀保留与名字校验都在 Rust。
    const result = await afterEditorsSaved(
      () => commands.vaultRename(currentPath, wantedName),
      () => ({ status: "error" as const, error: tMenu("common:saveBlocksMove") }),
    );
    if (result.status === "error") {
      setRenameError(vaultErrorText(result.error));
      onStatus(t("error"));
      return;
    }
    const renamedPath = result.data.path;
    announceRelocation(result.data.receipt);
    // 改完名要**重读一次正文**：搬动会跟随改写这一篇自己的 frontmatter（录音笔记的
    // `media[]` 库相对路径），不重读的话本地"已保存内容"与磁盘对不上，编辑器会立刻
    // 把它判成"有未保存改动"。
    const reread = await commands.readNote(renamedPath);
    pathRef.current = renamedPath;
    if (reread.status === "ok") {
      savedContentRef.current = reread.data.content;
    }
    setNameDraft(displayNameFromPath(renamedPath));
    setRenameError(null);
    markDirty(false);
    onSavedPath?.(renamedPath, currentPath);
    onStatus("");
  }

  // ── 时间戳行表 → 编辑器（PRD-04-066）────────────────────────────────────────

  const hasTimecodeLines = timecodeLines.length > 0;

  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    view.dispatch({
      effects: [
        // 有行才装装订线，没有就整个摘掉——空装订线在没有时间戳的笔记上就是一条白边。
        timecodeCompartmentRef.current.reconfigure(
          hasTimecodeLines
            ? timecodeGutterExtension({
                onSeek: (ms) => timecodeSeekRef.current(ms),
                playLabel: () => timecodePlayLabelRef.current,
              })
            : [],
        ),
        setTimecodeLinesEffect.of(
          new Map(timecodeLines.map((entry) => [entry.line, entry.startMs])),
        ),
      ],
    });
  }, [timecodeLines, hasTimecodeLines]);

  // ── 当前行高亮（宿主驱动，与回放视图同源） ────────────────────────────────
  //
  // 只有一个来源：宿主正在放**本篇**时的当前行。此前还有一个 `cueHighlightLine`
  // 属性，从写下那天起全仓就没有任何调用方传过值（恒 `undefined`）——它是上一次尝试
  // 留下的半截路，本轮把这条路真正走通之后已无存在理由，随本次一并删除。
  const liveCueLine = (() => {
    if (!playingThisNote || timecodeLines.length === 0) return null;
    const index = activeTimecodeIndex(timecodeLines, playbackHost?.playbackState.positionMs ?? 0);
    // 行号在 Rust 侧是 1-based，高亮效果吃的是 0-based。
    return index < 0 ? null : (timecodeLines[index]?.line ?? 1) - 1;
  })();

  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    const line = liveCueLine;
    view.dispatch({
      effects: highlightCueLineEffect.of(line),
    });
    // 跟随（默认关，由「正在播放」条上的开关打开）：把当前句滚进视野中部。
    // 与高亮**同一次 dispatch 之后**再滚：先让装饰落地，再按落地后的文档量位置。
    if (line !== null && playbackHost?.followInEditor) {
      const target = view.state.doc.line(
        Math.max(1, Math.min(line + 1, view.state.doc.lines)),
      );
      view.dispatch({ effects: EditorView.scrollIntoView(target.from, { y: "center" }) });
    }
  }, [liveCueLine, playbackHost?.followInEditor]);

  const onWindowCloseRef = useRef(onWindowClose);
  onWindowCloseRef.current = onWindowClose;

  // 关闭前收尾：落盘 + 弹出窗口回主窗打开。
  // **不手动 close()、不 preventDefault（成功路径）**：Tauri `onCloseRequested` 包装器
  // 在处理器返回且未 preventDefault 时自行 `window.destroy()` 真正关窗（见 @tauri-apps
  // /api window.js）——弹出窗口经 `note-editor` 能力授 allow-destroy 得以关闭；主窗未授
  // 该权限，其 destroy 失败、由 Rust 端 prevent_close + hide 接管（红叉隐藏，退出走托盘）。
  useEffect(() => {
    const unlisten = getCurrentWindow().onCloseRequested(async (event) => {
      // 有未保存改动先落盘；保存失败则取消关闭，让用户处理错误。
      const saved = await withEditorSaveGuard(async () => {
        try {
          await onWindowCloseRef.current?.();
        } catch {
          // 收尾副作用失败不应把窗口卡在打开态。
        }
      });
      if (!saved) {
        event.preventDefault();
      }
    });

    return () => {
      void unlisten.then((dispose) => {
        dispose();
      });
    };
  }, []);

  /** 命令面板条目 id → 本编辑器的动作（见 `EDITOR_PALETTE_COMMANDS`；未知 id 忽略）。 */
  const editorCommandRef = useRef<(id: string) => void>(() => {});
  editorCommandRef.current = (id) => {
    switch (id) {
      case "toggle_reading_view":
        changeMode(toggledViewMode(mode, lastEditModeRef.current));
        break;
      case "view_live":
        changeMode("live");
        break;
      case "view_source":
        changeMode("source");
        break;
      case "view_reading":
        changeMode("reading");
        break;
      case "read_aloud":
        if (ttsAvailable) {
          toggleReadDocument();
        }
        break;
      case "version_history":
        if (!restricted) {
          setVersionHistoryOpen(true);
        }
        break;
      case "ai_doc":
        if (!restricted) {
          openDocSession();
        }
        break;
      case "ai_composer":
        toggleAiComposer();
        break;
      case "typewriter_mode":
        setTypewriterMode(!typewriterMode);
        void commands.setSetting("editor.typewriter_mode", { kind: "bool", value: !typewriterMode });
        break;
      case "word_goal":
        void promptWordGoal();
        break;
      case "tab_focus_mode":
        toggleTabFocusMode();
        break;
      case "spellcheck":
        setSpellcheck(!spellcheck);
        void commands.setSetting("editor.spellcheck", { kind: "bool", value: !spellcheck });
        break;
      case "focus_mode":
        setFocusMode(!focusMode);
        void commands.setSetting("editor.focus_mode", { kind: "bool", value: !focusMode });
        break;
      case "format_toolbar":
        setShowFormatToolbar(!showFormatToolbar);
        void commands.setSetting("editor.show_format_toolbar", {
          kind: "bool",
          value: !showFormatToolbar,
        });
        break;
      case "view_in_graph":
        if (!restricted) {
          openInGraph();
        }
        break;
      case "insert_template":
        if (!restricted) {
          runFormatAction({ kind: "template" });
        }
        break;
      case "generate_graph_tags":
        if (!restricted && !graphTagsBusy) {
          void generateGraphTags();
        }
        break;
      default:
        break;
    }
  };

  /**
   * 编辑器宽度档（`editor-tier.ts`）：页头按钮形态、「更多」菜单内容与格式工具栏按同一
   * 档位降级。未量到（首个 layout effect 之前）按 wide 处理——那一帧不会被绘制。
   */
  const inlineSize = useInlineSize(sectionRef);
  const tier = editorTierFor(inlineSize ?? EDITOR_TIER_WIDE_MIN);
  /** 编辑器两向尺寸：气泡与 AI 面板的翻转 / 夹回判定要真实高度。 */
  const editorBox = useBoxSize(sectionRef);

  /**
   * 页头「更多」菜单（第四段）。低频项收在这里，走全应用唯一的菜单宿主（Esc / 方向键 /
   * 焦点归还都有）：
   * · 打字机 / 专注——设备级写作偏好（PRD-06-014），勾选项 + 悬停解释，与设置页仍是双入口；
   *   阅读模式下不适用，故不列（不是列成禁用）；
   * · 在关系图谱中查看 / 更新关系标签（AI）——图谱入口（PRD-06-034/035），副库窗口不列
   *   （落点是主库）。
   * 标签条右端的「向右分屏 / 移到新窗口」**不**在这里重复（PRD-07-065：同一事实同屏只
   * 呈现一次；它们是 pane 级动作，归标签条）。所有项都不适用时按钮整个不渲染——一个打开
   * 是空的菜单只是噪声。
   */
  const moreMenuItems = buildMenu([
    // narrow 档：全文朗读与版本历史从页头收进来（页头只留 AI 与「更多」）。
    tier === "narrow" &&
      ttsAvailable && {
        kind: "action" as const,
        id: "readDoc",
        label: readActive ? t("read.stopDoc") : t("read.readDoc"),
        icon: readActive ? <SpeakingIcon size={13} /> : <SpeakIcon size={13} />,
        onSelect: toggleReadDocument,
      },
    tier === "narrow" &&
      !restricted && {
        kind: "action" as const,
        id: "versionHistory",
        label: t("version.open"),
        icon: <ClockIcon size={13} />,
        onSelect: () => {
          setVersionHistoryOpen(true);
        },
      },
    separator("docActions"),
    mode !== "reading" && {
      kind: "action" as const,
      id: "typewriter",
      label: t("writing.typewriter"),
      title: t("writing.typewriterHint"),
      checked: typewriterMode,
      onSelect: () => {
        const next = !typewriterMode;
        setTypewriterMode(next);
        void commands.setSetting("editor.typewriter_mode", {
          kind: "bool",
          value: next,
        });
      },
    },
    mode !== "reading" && {
      kind: "action" as const,
      id: "focus",
      label: t("writing.focus"),
      title: t("writing.focusHint"),
      checked: focusMode,
      onSelect: () => {
        const next = !focusMode;
        setFocusMode(next);
        void commands.setSetting("editor.focus_mode", { kind: "bool", value: next });
      },
    },
    mode !== "reading" && {
      kind: "action" as const,
      id: "spellcheck",
      label: t("writing.spellcheck"),
      title: t("writing.spellcheckHint"),
      checked: spellcheck,
      onSelect: () => {
        const next = !spellcheck;
        setSpellcheck(next);
        void commands.setSetting("editor.spellcheck", { kind: "bool", value: next });
      },
    },
    // 工具栏显隐（editor.show_format_toolbar）：阅读模式没有工具栏，不列。
    mode !== "reading" && {
      kind: "action" as const,
      id: "formatToolbar",
      label: t("toolbar.show"),
      checked: showFormatToolbar,
      onSelect: () => {
        const next = !showFormatToolbar;
        setShowFormatToolbar(next);
        void commands.setSetting("editor.show_format_toolbar", { kind: "bool", value: next });
      },
    },
    mode !== "reading" && path?.endsWith(".md") && {
      kind: "action" as const,
      id: "wordGoal",
      label: t("wordGoal.menu"),
      title: wordGoal === null ? t("wordGoal.desc") : t("wordGoal.descCurrent", { goal: wordGoal }),
      onSelect: () => void promptWordGoal(),
    },
    separator("writing"),
    !restricted && {
      kind: "action" as const,
      id: "viewGraph",
      label: t("graphTags.viewGraph"),
      onSelect: openInGraph,
    },
    !restricted && {
      kind: "action" as const,
      id: "generateGraphTags",
      label: graphTagsBusy ? t("graphTags.generating") : t("graphTags.generate"),
      disabled: graphTagsBusy,
      onSelect: () => {
        void generateGraphTags();
      },
    },
  ]);


  return (
    <section
      ref={sectionRef}
      className="luna-paper luna-editor"
      data-empty={path === null}
      /* 悬挂缩进按模式选量纲（editor-core.css `--md-hang-source` 选择器）。 */
      data-view-mode={mode}
      data-tier={tier}
    >
      <header className="luna-main-header luna-editor__header">
        {path ? (
          <div className="luna-main-header__title">
            {!isFolder ? (
              <input
                ref={nameInputRef}
                type="text"
                className="luna-editor__title-input"
                value={nameDraft}
                aria-label={t("nameLabel")}
                placeholder={t("namePlaceholder")}
                onChange={(event) => {
                  setNameDraft(event.target.value);
                }}
                onKeyDown={(event) => {
                  if (isImeKeyEvent(event)) return;
                  if (event.key === "Enter") {
                    event.preventDefault();
                    void renameCurrentNote().then(() => {
                      viewRef.current?.focus();
                    });
                  }
                }}
                onBlur={() => {
                  void renameCurrentNote();
                }}
              />
            ) : (
              <span className="luna-editor__folder-title">
                <FolderIcon size={18} className="luna-editor__title-icon-folder" />
                {nameDraft}
              </span>
            )}
          </div>
        ) : (
          <div>
            {/* 标题说**当前是什么状况**，不复述这个面板叫什么名字——这里此前写的是
                「编辑器」，那正是 `EmptyState` 抬头明令禁止的那种标题（真机 2026-09-08
                上报「速记页跟别的页风格不统一」，最刺眼的就是这一处）。 */}
            <h2>{t(`canvas.${canvasDestination}.title`)}</h2>
            {/* 空态话术跟着目的地走（PRD-06-058 P1-1）：笔记库的通用话术（"直接输入创建
                新笔记"）在记录语境下不成立——它承诺的落点与这一格实际的落点相反。 */}
            <span>{t(`canvas.${canvasDestination}.empty`)}</span>
          </div>
        )}
        <div className="luna-main-header__actions luna-editor__actions">
          {path && !isFolder && (
            <>
              {/* ── 第一段：身份与能力 ──────────────────────────────────────────
                  沉淀入口（PRD-06-044）：录音 / 转写从文件树点开走的正是这里。「来源层」身份
                  不再挂徽标，其待遇改在上下文面板「信息」页签说明（PRD-06-048②）。整段在副库窗口不渲染（PRD-06-050）：徽标查询与蒸馏动作都以
                  主库为落点，在副库语境下给出的是另一个库的答案。 */}
              {!restricted && (
                <div className="luna-editor__actions-group">
                  {/* 从文件树点开一篇录音时，这里是通往同步回放的唯一线索（PRD-04-020/023）。 */}
                  <PlaybackEntryBadge notePath={path} onSectionChange={setPlaybackSection} />
                  {/* 记录身份 + 一键沉淀（PRD-06-052/053）：与沉淀入口并列同一格——它们答的是
                      同一个问题（这一篇算哪一层），只是两根轴上的两个答案，且三态互斥，
                      故绝不会同时出现两枚。它与下面那枚「生成知识笔记」（AI 辅助沉淀，
                      PRD-06-054）**同时**出现在记录态笔记上：一个原地去键、一个另出一篇，
                      裁决与理由见 `DistillBadge`。 */}
                  <DraftBadge
                    notePath={path}
                    onSettled={(target: string) => {
                      openNote(target);
                    }}
                  />
                  {/* 主题页身份与所有权（PRD-16-021）。 */}
                  <DistillBadge
                    notePath={path}
                    onOpenNote={(target: string) => {
                      openNote(target);
                    }}
                  />
                </div>
              )}
              {/* **保存态与字数只在状态栏说一次**：它们此前在页头、状态栏右侧各有一份常驻
                  呈现，加上通用状态通道被灌进 `clean`，同一件事同屏出现三次、字数两次且
                  措辞还不一样（「字数: N」vs「N 字」）。分工：页头 = 这一篇的**身份与动作**，
                  状态栏 = 当前活跃文档的**量化事实与状态**；「哪一个标签有改动」另由标签页
                  圆点回答（三者各答一个问题，不是同一句话说三遍）。 */}
              {/* ── 第二段：视图三态（PRD-06-017）──────────────────────────────
                  页头**唯一**的段控。此前打字机/专注（两态开关）与图谱/标签（两个动作）
                  也各穿一副同样的段控皮肤并排在旁边——三组长得一样、语义三种，用户分不清
                  哪组是互斥单选、哪组是开关。前者收进「更多」菜单（勾选项，文字自带解释），
                  后者同去（低频入口不占页头黄金位）。阅读模式的快捷键提示从注册表派生。 */}
              <div className="luna-editor__mode-toggle" role="group" aria-label={t("mode.label")}>
                <IconButton
                  aria-pressed={mode === "live"}
                  label={t("mode.live")}
                  icon={<PencilIcon size={13} />}
                  onClick={() => changeMode("live")}
                />
                <IconButton
                  aria-pressed={mode === "source"}
                  label={t("mode.source")}
                  icon={<CodeIcon size={13} />}
                  onClick={() => changeMode("source")}
                />
                <IconButton
                  aria-pressed={mode === "reading"}
                  label={
                    readingShortcut
                      ? t("toolbar.labelShortcut", {
                          label: t("mode.reading"),
                          shortcut: formatShortcut(readingShortcut),
                        })
                      : t("mode.reading")
                  }
                  icon={<BookOpenIcon size={13} />}
                  onClick={() => changeMode("reading")}
                />
              </div>
              {/* ── 第三段：对这一篇的动作（AI · 朗读 · 版本历史 · 保存 · 更多）────── */}
              <div className="luna-editor__actions-group">
                {/* 三档降级（`editor-tier.ts`）：wide = 文字钮；compact = 图标钮（名字进提示）；
                    narrow = 朗读与版本历史收进「更多」，页头只留 AI（这一篇最有产品辨识度的
                    动作）与「更多」。三档渲染的是**同一批动作**，只是形态不同——不会有哪个
                    宽度下某个动作消失。 */}
                {!restricted &&
                  (tier === "wide" ? (
                    <button
                      type="button"
                      className="luna-btn luna-btn--secondary luna-btn--sm luna-editor__ai-trigger"
                      title={t("floating.trigger_title")}
                      onClick={() => openDocSession()}
                    >
                      <SparklesIcon size={14} />
                      {t("floating.trigger")}
                    </button>
                  ) : (
                    <IconButton
                      className="luna-editor__ai-trigger"
                      label={t("floating.trigger_title")}
                      icon={<SparklesIcon size={14} />}
                      onClick={() => openDocSession()}
                    />
                  ))}
                {ttsAvailable && tier === "wide" ? (
                  <button
                    type="button"
                    className="luna-btn luna-btn--secondary luna-btn--sm"
                    aria-pressed={readActive}
                    data-speaking={readActive || undefined}
                    title={readActive ? t("read.stopDoc") : t("read.readDoc")}
                    onClick={toggleReadDocument}
                  >
                    {readActive ? <SpeakingIcon size={14} /> : <SpeakIcon size={14} />}
                    {t("read.readDoc")}
                  </button>
                ) : null}
                {ttsAvailable && tier === "compact" ? (
                  <IconButton
                    aria-pressed={readActive}
                    data-speaking={readActive || undefined}
                    label={readActive ? t("read.stopDoc") : t("read.readDoc")}
                    icon={readActive ? <SpeakingIcon size={14} /> : <SpeakIcon size={14} />}
                    onClick={toggleReadDocument}
                  />
                ) : null}
                {!restricted && tier === "wide" && (
                  <button
                    type="button"
                    className="luna-btn luna-btn--secondary luna-btn--sm"
                    onClick={() => {
                      setVersionHistoryOpen(true);
                    }}
                  >
                    {t("version.open")}
                  </button>
                )}
                {!restricted && tier === "compact" && (
                  <IconButton
                    label={t("version.open")}
                    icon={<ClockIcon size={14} />}
                    onClick={() => {
                      setVersionHistoryOpen(true);
                    }}
                  />
                )}
                {/* 自动保存开着时**不渲染**它：`disabled={!dirty}` 且自动保存把 dirty 压在
                    防抖窗口内，于是它是一颗几乎永远点不动的按钮，占着页头最贵的一格。
                    关掉自动保存后它是唯一的落盘入口，那时才必须在场。 */}
                {!autosaveEnabled && (
                  <button
                    type="button"
                    className="luna-btn luna-btn--secondary luna-btn--sm"
                    disabled={!dirty}
                    onClick={() => {
                      void flushRef.current();
                    }}
                  >
                    {t("save")}
                  </button>
                )}
                {/* ── 第四段：「更多」菜单（条目见 `moreMenuItems`）─────────────────── */}
                {moreMenuItems.length > 0 && (
                  <IconButton
                    label={t("more.label")}
                    icon={<EllipsisIcon size={14} />}
                    aria-haspopup="menu"
                    onClick={(event) => {
                      contextMenu.open(event, moreMenuItems, { label: t("more.label") });
                    }}
                  />
                )}
              </div>
            </>
          )}
        </div>
      </header>
      {path && !isFolder && mode !== "reading" && showFormatToolbar ? (
        <FormatToolbar formatState={formatState} onAction={runFormatAction} tier={tier} />
      ) : null}
      {renameError ? (
        <div className="luna-editor__conflict" role="status">
          <span>{renameError}</span>
        </div>
      ) : null}
      {loadError && path ? (
        <div className="luna-editor__conflict" role="alert">
          <span>{t("loadFailedNotice", { error: loadError })}</span>
          <button type="button" onClick={() => setLoadAttempt((attempt) => attempt + 1)}>
            {t("loadRetry")}
          </button>
        </div>
      ) : null}
      {externalPending && (
        <div className="luna-editor__conflict" role="status">
          <span>{t(externalContentRef.current === null ? "externalDeleted" : "externalConflict")}</span>
          <button type="button" disabled={externalContentRef.current === null} onClick={() => { void mergeExternalContent(); }}>
            {t("conflictMerge")}
          </button>
          <button type="button" disabled={externalContentRef.current === null} onClick={() => { void reloadExternalContent(); }}>
            {t("reload")}
          </button>
          <button type="button" disabled={externalContentRef.current === null} onClick={keepLocalContent}>
            {t("keepLocal")}
          </button>
          <button type="button" onClick={() => { void saveConflictCopy(); }}>{t("conflictSaveCopy")}</button>
          <button type="button" onClick={compareExternalContent}>{t("conflictCompare")}</button>
        </div>
      )}
      {externalPending && conflictDiff ? <div className="luna-editor__conflict-diff"><DiffView diff={conflictDiff} /></div> : null}
      {path && !isFolder ? (
        <ConflictCopyBanner path={path} currentText={currentDocumentText} onOpenPath={onOpenPath} onStatus={onStatus} />
      ) : null}
      {pastePreview ? <PastePreviewDialog {...pastePreview} onResolve={resolvePaste} /> : null}
      {templateDialogOpen ? (
        <TemplateInsertDialog
          onPick={(template) => resolveTemplate(template)}
          onClose={() => resolveTemplate(null)}
          onError={onStatus}
        />
      ) : null}
      {/* AI 落地反馈（PRD-06-021）：与外部冲突横幅同一条带、同一形态——页头下方的非模态
          横条，不挡正文、不夺焦点；几秒后自动收起，「撤销」走编辑器撤销栈。 */}
      {landed ? (
        <AiLandedToast
          message={t(landed.landing === "replace" ? "floating.landed.replace" : "floating.landed.insert")}
          onUndo={undoLanded}
          onDismiss={dismissLanded}
        />
      ) : null}
      {tidyPreview ? (
        <TidyListDialog
          preview={tidyPreview}
          onCancel={() => {
            setTidyPreview(null);
          }}
          onConfirm={() => {
            void applyTidyListLevels();
          }}
        />
      ) : null}
      {versionHistoryOpen ? (
        <VersionHistoryPanel
          path={pathRef.current ?? path}
          onClose={() => {
            setVersionHistoryOpen(false);
          }}
        />
      ) : null}
      {readActive ? (
        <ReadAloudControls
          state={ttsState}
          ratePct={speechRatePct}
          onPauseResume={toggleReadPauseResume}
          onStop={() => {
            void ttsStop();
          }}
          onRateChange={changeReadRate}
        />
      ) : null}
      {bubble && !aiSession && !isFolder && !restricted && mode !== "reading" && editorBox ? (
        <SelectionBubble
          box={bubble.box}
          editorSize={{ width: editorBox.inline, height: editorBox.block }}
          formatState={formatState}
          onFormatAction={runFormatAction}
          aiTitle={
            aiShortcut
              ? t("floating.bubble.ai_title", { shortcut: formatShortcut(aiShortcut) })
              : t("menu.aiActions")
          }
          onAi={() => {
            if (selectionState) {
              openSelectionSession(selectionState);
            }
          }}
          onRead={ttsAvailable ? readSelectionAloud : null}
          onRelated={() => {
            if (selectionState) {
              openSelectionSession(selectionState, { search: true });
            }
          }}
          onQuote={
            !quoteAvailable || noteTypeRef.current === "script" || noteTypeRef.current === "video-project"
              ? null
              : saveQuote
          }
        />
      ) : null}
      {aiSession && !isFolder && !restricted && editorBox ? (
        <FloatingMenu
          aiShortcut={aiShortcut}
          scope={aiSession.scope}
          selectedText={aiSession.text}
          context={aiSession.context}
          notePath={pathRef.current ?? path}
          noteType={noteType}
          anchorBox={aiSession.box}
          editorSize={{ width: editorBox.inline, height: editorBox.block }}
          initialSearch={aiSession.initialSearch}
          pending={aiSession.pending}
          tableTask={aiSession.tableTask ?? null}
          autoRun={aiSession.autoRun ?? null}
          contextEnabled={contextEnabled}
          onToggleContext={toggleContextSetting}
          onLand={landAiResult}
          onOpenPath={onOpenPath}
          onClose={closeAiSession}
          onGraphTags={runGraphTags}
          onViewGraph={openInGraph}
          controlsRef={aiControlsRef}
        />
      ) : null}
      {isFolder ? (
        <div className="luna-editor__folder-placeholder">
          <FolderIcon size={48} className="luna-editor__folder-placeholder-icon" />
          <h3>{nameDraft}</h3>
          {folderStats ? (
            <div className="luna-editor__folder-stats">
              <span className="luna-editor__folder-stat-badge">
                <strong>{folderStats.folders}</strong> {t("foldersCount")}
              </span>
              <span className="luna-editor__folder-stat-divider">•</span>
              <span className="luna-editor__folder-stat-badge">
                <strong>{folderStats.files}</strong> {t("filesCount")}
              </span>
            </div>
          ) : (
            <p>{t("folderPlaceholder")}</p>
          )}
        </div>
      ) : null}
      {!isFolder && mode === "reading" && path ? (
        <div className="luna-editor__reading-scroll" data-doc-version={docVersion}>
          <ReadingView
            content={viewRef.current?.state.doc.toString() ?? savedContentRef.current}
            notePath={path}
            onOpenPath={onOpenPath}
            onOpenTag={onOpenTag}
            onActivateWikiLink={activateWikiLink}
          />
        </div>
      ) : null}
      {/* 速记产物条（PRD-02-033）：只在语音速记上出现，由 Rust 判定类型；副库窗口不给。 */}
      {path && !isFolder && !restricted ? <QuickNoteProductBar notePath={path} /> : null}
      {/* CM 宿主常驻挂载（EditorView 只建一次）：文件夹/阅读态/没装载成功仅视觉隐藏。 */}
      <div
        ref={hostRef}
        className="luna-editor__surface"
        data-hidden={isFolder || mode === "reading" || (loadError !== null && path !== null) || undefined}
        data-focus={(focusMode && mode !== "reading") || undefined}
        onContextMenu={openEditorContextMenu}
      />
      <LinkPreviewPopover controller={linkPreview} onOpenPath={(target) => onOpenPath?.(target)} />
    </section>
  );
}
