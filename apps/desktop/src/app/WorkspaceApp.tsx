import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
  type SetStateAction,
} from "react";
import { afterEditorsSaved, withEditorSaveGuard } from "../features/editor/editor-save-guard";
import { cssVariables } from "../lib/css-variables";
import { IconButton } from "../components/ui/IconButton";
import { LunaLogo } from "../components/ui/LunaLogo";
import { requestEditorReveal, requestKnowledgeCheckup } from "../lib/app-events";
import {
  commands,
  events,
  type NoteTreeDto,
  type NewProjectOptionsDto,
  type DraftFilterDto,
  type DraftInboxDto,
  type DraftNoteDto,
  type ListSortDto,
  type SearchHitDto,
  type TopicDirectoryRowDto,
  type TrashEntryDto,
  type VaultStatusDto,
} from "../ipc/bindings";
import { ChatView } from "../features/chat/ChatView";
import { AppsView } from "../features/apps/AppsView";
import { TeleprompterHome } from "../features/teleprompter/TeleprompterHome";
import { createTeleprompterDraft, openInTeleprompter } from "../features/teleprompter/teleprompter-actions";
import { CreatorBoard } from "../features/creator/CreatorBoard";
import { CreatorNotePage } from "../features/creator/CreatorNotePage";
import { NewProjectDialog } from "../features/creator/NewProjectDialog";
import { ScriptProjectBanner } from "../features/creator/ScriptProjectBanner";
import { ProjectWorkspace } from "../features/creator/ProjectWorkspace";
import {
  creatorActivePath,
  creatorViewOf,
  remapCreatorView,
  type CreatorView,
} from "../features/creator/creator-view";
import { PublishView } from "../features/creator/PublishView";
import { SwipeLibrary } from "../features/creator/SwipeLibrary";
import { CreatorSidebar } from "../features/creator/CreatorSidebar";
import { useCreatorBoard } from "../features/creator/use-creator-board";
import type { ProjectMenuNav } from "../features/creator/project-menu";
import type { ContextMenuEntry } from "../lib/context-menu";
import { formatClock } from "../lib/format-duration";
import { ChatHistoryList } from "../features/chat/ChatHistoryList";
import { useChatController } from "../features/chat/use-chat-controller";
import { useChatSessionSelector } from "../features/chat/use-chat-session";
import {
  type CommandContext,
  workspaceCommandForKey,
  EDITOR_KEYED_SETTING_KEYS,
} from "../features/command/command-registry";
import { useShortcutBindings } from "../hooks/use-shortcut-bindings";
import { DoubleTapDetector } from "../lib/double-tap";
import { QuickSwitcher } from "../features/command/QuickSwitcher";
import { CommandPalette } from "../features/command/CommandPalette";
import { EMPTY_EDITOR_METRICS, VaultEditor, type EditorMetrics } from "../features/editor/VaultEditor";
import { setTabFocusMode, useTabFocusMode } from "../lib/tab-focus-mode";
import { AssetView } from "../features/media/AssetView";
import { classifyAssetTarget } from "@lunarca/editor-core/asset-kind";
import { DashboardView } from "../features/dashboard/DashboardView";
import { LAUNCH_OVERVIEW_KEY } from "../features/dashboard/launch-overview";
import { SetupBanner } from "../features/onboarding/SetupBanner";
import { projectSetupBanner, useSetupStatusContext } from "../features/onboarding/use-setup-status";
import { TrashView } from "../features/workspace/TrashView";
import {
  useDeleteConsequence,
  useDeleteMode,
} from "../hooks/use-delete-mode";
import { useDesktopLens } from "../features/popup-translate/use-desktop-lens";
import { useTranslation } from "react-i18next";
import { SettingsView, type SettingsPagePanels } from "../features/settings/SettingsView";
import { CaptionSettingsPreview } from "../features/caption/CaptionSettingsPreview";
import { AccountPanel } from "../features/account/AccountPanel";
import { SettingsNav } from "../features/settings/SettingsNav";
import { useSettingsController } from "../features/settings/use-settings-controller";
import { formatShortcut } from "@lunarca/editor-core/shortcut-display";
import {
  recordingIndicatorFrom,
  type RecordingIndicator,
} from "../features/workspace/recording-indicator";
import { requestDistillNotes } from "../features/distill/distill-actions";
import { FilteredNoteList } from "../features/workspace/FilteredNoteList";
import { useNoteSelection } from "../features/workspace/use-note-selection";
import { useVaultReload } from "../hooks/use-vault-reload";
import { useBoolSetting } from "../hooks/use-bool-setting";
import { useApps } from "../hooks/use-apps";
import { DraftInbox } from "../features/capture/DraftInbox";
import { QuickNoteActions } from "../features/capture/QuickNoteActions";
import { useNoteFolders, type NoteFolders } from "../features/workspace/use-note-folders";
import { SyncPlaybackView } from "../features/playback/SyncPlaybackView";
import { PlaybackProvider } from "../features/playback/playback-host";
import { TranscribeNoteList } from "./TranscribeNoteList";
import { TranscribeView } from "../features/transcribe/TranscribeView";
import { UrlImportDialog } from "../features/transcribe/UrlImportDialog";
import { useTranscribeImport } from "../features/transcribe/use-transcribe-import";
import { useMediaDrop } from "../features/transcribe/use-media-drop";
import { TemplatePicker } from "../features/workspace/TemplatePicker";
import { ContextPanel } from "../features/workspace/ContextPanel";
import { TagTreePanel, TagNotesView } from "../features/workspace/TagsView";
import { useTagBrowser } from "../features/workspace/use-tag-browser";
import { SearchView } from "../features/search/SearchView";
import { useVaultSearch } from "../features/search/use-vault-search";
import {
  canvasDestination,
  hasEditorSurface,
  hasSidebar,
  locationFor,
  sameLocation,
  sidebarKeyOf,
  spaceLabelKey,
  SIDEBAR_WIDTH_MAX,
  SIDEBAR_WIDTH_MIN,
  type NavBadge,
  type SidebarKey,
  type WorkspaceLocation,
  type WorkspaceSection,
  type WorkspaceSpace,
  surfaceOf,
} from "../features/workspace/workspace-ia";
import { parseWorkspaceRoute } from "../features/workspace/workspace-routes";
import { pageOf, usePagedList } from "../features/workspace/paged-list";
import { useDebouncedValue } from "../hooks/use-debounced-value";
import { ListLoadMore } from "../features/workspace/ListLoadMore";
import { CaptureStartButton } from "../components/ui/CaptureStartButton";
import {
  DEFAULT_RECORDING_SORT,
  DEFAULT_TRANSCRIBE_SORT,
  timeGroupingFor,
  rowTimeFor,
  formatListDuration,
} from "../features/transcribe/transcribe-list";
import { WorkspacePrimaryRail } from "../features/workspace/WorkspacePrimaryRail";
import { WorkspaceSectionNav } from "../features/workspace/WorkspaceSectionNav";
import { useRailLayout } from "../features/workspace/use-rail-layout";
import { KnowledgeWorkbench } from "../features/knowledge/KnowledgeWorkbench";
import { KnowledgeReadingView } from "../features/workspace/KnowledgeReadingView";
import { TopicsView } from "../features/knowledge/TopicsView";
import { AddContentPopover } from "../features/capture/AddContentPopover";
import type { CreateActionContext } from "../features/capture/create-actions";
import { useSurfaceMetrics } from "../features/workspace/use-workspace-metrics";
import {
  ArrowClockwiseIcon,
  CameraIcon,
  CaptionsIcon,
  ChevronRightIcon,
  CloseIcon,
  DiscIcon,
  FolderIcon,
  HardDriveIcon,
  InfoIcon,
  LanguagesIcon,
  MicIcon,
  PlusIcon,
  SearchIcon,
  SettingsIcon,
  SidebarCloseIcon,
  SidebarOpenIcon,
} from "@lunarca/editor-core/Icon";
import { EmptyState } from "../components/ui/EmptyState";
import { useDialogBehavior } from "../hooks/use-dialog-behavior";
import { GraphView } from "../features/graph/GraphView";
import { GraphSidebar } from "../features/graph/GraphSidebar";
import { GraphRescanDialog } from "../features/graph/GraphRescanDialog";
import { DistillBatchDialog } from "../features/distill/DistillBatchDialog";
import { CheckupDialog } from "../features/knowledge/CheckupDialog";
import { TopicDraftHost } from "../features/knowledge/TopicDraftHost";
import { VaultSwitcher } from "../features/vault/VaultSwitcher";
import { VaultManagerDialog } from "../features/vault/VaultManagerDialog";
import { VaultEmptyActions } from "../features/vault/VaultEmptyActions";
import { VaultSwitchDialog } from "../features/vault/VaultSwitchDialog";
import { useVaultLinks } from "../features/workspace/use-vault-links";
import { usePrompt } from "../components/PromptHost";
import { useVaultSwitch } from "../features/vault/use-vault-switch";
import { DistillPreviewDialog } from "../features/distill/DistillPreviewDialog";
import { useDistillPreview } from "../features/distill/use-distill-preview";
import { useDistillBatch } from "../features/distill/use-distill-batch";
import { useGraphController } from "../features/graph/use-graph-controller";
import { ProcessingQueueEntry } from "../features/processing/ProcessingQueueEntry";
import { useStatusVisitMetrics } from "../hooks/use-status-visit-metrics";
import {
  activeCount,
  currentProcessingQueue,
  useProcessingQueueSelector,
} from "../features/processing/use-processing-queue";
import { NoteTree } from "../features/workspace/NoteTree";
import { useContextMenu } from "../components/ContextMenuHost";
import {
  StatusBarCompute,
  StatusBarMessage,
  StatusBarNotifications,
  StatusBarRecording,
  StatusBarResource,
  StatusBarSync,
} from "./StatusBarIndicators";
import { ArchiveRowMeta } from "./ArchiveRowMeta";
import { deleteNotesWithConfirm, useNoteMenu } from "../features/workspace/note-menu";
import { useConfirm, type ConfirmOptions } from "../components/ConfirmHost";
import { isTaskActive } from "../features/processing/processing-labels";
import { ArchiveListFilters } from "../features/transcribe/ArchiveListFilters";
import { withoutErrorCode } from "../lib/task-failure";
import { noteStem } from "../lib/note-path";
import { NoteTabsBar } from "../features/workspace/NoteTabsBar";
import { RelocationReceiptBar, useRelocationReceipt } from "../features/workspace/RelocationReceiptBar";
import { ExternalRenameBar, useExternalRenamePrompt } from "../features/workspace/ExternalRenameBar";
import { ObsidianSuggestionBar, useObsidianSuggestion } from "../features/workspace/ObsidianSuggestionBar";
import {
  activePane as activePaneOf,
  activeTab as activeNoteTab,
  activeTabOf,
  activateTab,
  closePane,
  closeTab,
  closeOtherTabs,
  closeRightTabs,
  closeTabsUnder,
  initialNotePanesState,
  MAX_PANES,
  moveTab,
  openEmptyTab,
  openNote,
  keepTab,
  remapPath,
  removeTab,
  reopenClosedTab,
  togglePinTab,
  restoreNotePanes,
  serializeNotePanes,
  setActivePane,
  setTabMode,
  goBack,
  goForward,
  splitPane,
  type NotePanesState,
  type OpenNoteOptions,
} from "../features/workspace/note-panes";
import type {
  AppId,
  VoiceInputHudPresetDto,
  WorkspaceMetricEntryResultDto,
} from "../ipc/bindings";
import { vaultErrorText } from "../lib/vault-error";
import { navigationHandled, NAVIGATE_EVENT } from "../lib/navigate";

function countFolderChildren(node: NoteTreeDto): { files: number; folders: number } {
  let files = 0;
  let folders = 0;
  for (const child of node.children) {
    if (child.is_dir) {
      folders += 1;
      const sub = countFolderChildren(child);
      files += sub.files;
      folders += sub.folders;
    } else {
      files += 1;
    }
  }
  return { files, folders };
}

function findNode(node: NoteTreeDto, path: string): NoteTreeDto | null {
  if (node.path === path) return node;
  for (const child of node.children) {
    const found = findNode(child, path);
    if (found) return found;
  }
  return null;
}

/**
 * 启动概览弹窗的进程内一次性标志（PRD-07-057 二次修订：仪表盘弹窗化）。
 *
 * module 级而非组件 state：切库会以 key 整体重挂载 WorkspaceShell（换库 = 换内容源），
 * 但"本次运行已看过概览"不该随之重置——重启应用才重置。
 */
let dashboardShownThisRun = false;

/** 转正撤销条挂多久（毫秒）。可逆动作值这几秒的安全感，但它不该赖着不走。 */
const UNDO_SETTLE_WINDOW_MS = 8_000;

/**
 * 整页由外部组件承载的设置页的组件表（跨 feature 组装归 `app/`，TDD-15 §2.2）：账户面板是
 * 另一个 feature，由这里交给设置中心，设置中心不反向依赖它。键按 IA 的 `SettingsPageId`
 * 收紧——写一个不存在的页 id 就是编译错误。
 */
const SETTINGS_PAGE_PANELS: SettingsPagePanels = {
  account: AccountPanel,
};

/**
 * 界面上会显示快捷键的那几条（取数与 `settings://changed` 订阅共用这一份清单）。
 *
 * 加一条要显示快捷键的按钮时，**只需要往这里加一行**：取数、订阅与消费面各自从这份
 * 清单派生，不会出现"state 加了、取数忘了"。放模块作用域而不是组件里——它是常量，留在
 * 组件里每帧新建一个数组，进依赖数组就等于每帧重跑那个 effect。
 */
const SHORTCUT_HINT_KEYS: readonly string[] = [
  "shortcuts.recording_toggle",
  "shortcuts.voice_input",
  "shortcuts.text_quick_note",
  "shortcuts.quick_chat",
  "shortcuts.quick_switcher",
  // 工作区命令的键位（W3：全部可改键，真值由 Rust 下发；命令注册表据此判定按键）。
  "shortcuts.command_palette",
  "shortcuts.vault_search",
  "shortcuts.toggle_sidebar",
  "shortcuts.open_settings",
  "shortcuts.reopen_closed_tab",
  // 编辑器整篇动作的键位（W3b）：命令面板里给它们标键要用真值。
  ...EDITOR_KEYED_SETTING_KEYS,
];

export function WorkspaceApp({
  onVaultSwitched,
  initialVault,
  launchOverview,
}: {
  onVaultSwitched?: () => void;
  /** 启动快照里的库状态（首次挂载）；`null` = 自己读（切库重挂载）。 */
  initialVault: VaultStatusDto | null;
  /** `appearance.launch_overview`（启动快照里读好的）。 */
  launchOverview: boolean;
}) {
  const { t } = useTranslation("workspace");
  const { t: tTranscribe } = useTranslation("transcribe");
  const { t: tCommon } = useTranslation("common");
  const { t: tChat } = useTranslation("chat");
  // 启动落点 = 笔记（PRD-07-057 修订 / 2026-07-28 用户定位裁决）：知识库是核心，
  /**
   * 用户此刻站在哪儿（ADR-087 ①）。**一级空间 + 二级分区**，不再是扁平目的地。
   *
   * 归一恒经 `locationFor`：分区不属于该空间时落回默认分区，故不存在"带着别的空间的
   * 分区"这种只在特定跳转顺序下才复现的状态。
   */
  const [location, setLocationRaw] = useState<WorkspaceLocation>(() => locationFor("notes"));
  const setLocation = useCallback((update: SetStateAction<WorkspaceLocation>) => {
    void withEditorSaveGuard(() => setLocationRaw(update));
  }, []);
  /** 最新位置（命令上下文是 memo 化的，侧栏开合的判据要读此刻而不是创建那一刻的位置）。 */
  const locationRef = useRef(location);
  locationRef.current = location;
  const space = location.space;
  const section = location.section;
  /** 去某个位置（不传分区 = 该空间的默认分区）。 */
  const go = useCallback((target: WorkspaceSpace, to?: WorkspaceSection) => {
    setLocation(locationFor(target, to));
  }, []);
  /** 此刻是不是正站在这个位置。 */
  const isAt = (target: WorkspaceSpace, to?: WorkspaceSection) =>
    sameLocation(location, locationFor(target, to));
  // 设置深链（`settings://<大类|分组>[/<key>]` 原样交给 SettingsView 解析；nonce 支持同目标重复导航）。
  const [settingsFocus, setSettingsFocus] = useState<{
    route: string;
    nonce: number;
  } | null>(null);
  // 设置控制器：主侧栏的分组导航与主区的设置面板共享同一实例（同 useChatController 范式），
  // 否则搜索词/当前分组/写入后的重读会各持一份而分叉。仅在进入设置目的地时取数。
  const settingsController = useSettingsController(space === "settings");
  // 标签树与主区清单共享唯一控制器：选择、摘要、笔记结果和 vault 刷新保持同一拍。
  const tagController = useTagBrowser(isAt("knowledge", "tags"));
  const [tree, setTree] = useState<NoteTreeDto | null>(null);
  /** 知识产物路径集（主题页 + 知识地图，ADR-096 ③）：Rust 已判定事实的缓存（与树同批
      随 `refreshVault` 重取），编辑桌与「笔记」投影据此把产物让路到知识库——前端不判定
      身份，只消费这份清单（ADR-003）。 */
  const [productPaths, setProductPaths] = useState<ReadonlySet<string>>(() => new Set());
  /** 导航事件监听器（`[]` deps）读不到最新 state——闸口判定经这面镜子取当前清单。 */
  const productPathsRef = useRef<ReadonlySet<string>>(new Set());
  /** 「知识库 → 主题页」当前选中的产物（目录行高亮；D2 起主区阅读视图渲染它）。 */
  const [topicSelection, setTopicSelection] = useState<string | null>(null);
  /**
   * 记录收件箱（PRD-06-058）：全库**记录态**笔记 + 权威总数。
   *
   * 取数从 `search({ note_type: "quick-note" })` 改成 `list_draft_notes`——收录口径
   * 由 type 驱动改为 **stage 驱动**（TDD-06 §5 功能视图口径的扩展）。总数与列表由同一次
   * 调用取齐，故导航徽标不会与点进去看到的行数对不上。
   */
  //
  // **整份信封原样存着**，不在这里挑几个字段重新拼一个形状：Rust 每多下发一位事实
  // （`can_merge` / `merge_min` / `merge_max` 就是这么来的），中间形状漏抄一次不会有任何
  // 东西变红——界面只是悄悄少了一条规则（ADR-003）。条目本身由分页控制器持有，故这里
  // 只留信封上的那几位（两个计数与合并限额，它们不随页变）。
  /** 「速记」侧栏快速捕获的展开请求序号（见 `captureTextQuickNote`）。 */
  const [captureOpenRequest, setCaptureOpenRequest] = useState(0);
  const [draftMeta, setDraftMeta] = useState<Omit<DraftInboxDto, "notes" | "next_cursor">>({
    total: 0,
    matched: 0,
    merge_min: 0,
    merge_max: 0,
  });
  const [selectedTranscribePath, setSelectedTranscribePath] = useState<string | null>(null);
  /** 「转写」清单的多选与可见行（口径同录音清单）。 */
  const [transcribeRows, setTranscribeRows] = useState<readonly string[]>([]);
  const transcribeSelection = useNoteSelection({
    current: selectedTranscribePath,
    visible: transcribeRows,
    onDelete: () => deleteTranscribeSelection(),
  });
  /**
   * 「转写」列表的筛选词与排序键。
   *
   * **它们归这里而不是那个列表组件**：排序与筛选已经整体下沉给 Rust（TDD-07 §4.1a），
   * 于是它们是**取数口径**的一部分——留在组件内部就只能作用于已加载的那几页。
   */
  const [transcribeQuery, setTranscribeQuery] = useState("");
  const [transcribeSort, setTranscribeSort] = useState<ListSortDto>(DEFAULT_TRANSCRIBE_SORT);
  /**
   * 真正送去取数的那个筛选词，比输入框慢一拍。
   *
   * 筛选下沉之后每敲一个字都是一次跨进程查询；不缓一下，打「周会」两个字就发三次，
   * 而只有最后那次的结果有意义。缓的是**取数**，不是输入框——输入框仍然逐字即时回显。
   */
  const [transcribeNeedle, setTranscribeNeedle] = useState("");
  useEffect(() => {
    const timer = window.setTimeout(() => setTranscribeNeedle(transcribeQuery.trim()), 200);
    return () => window.clearTimeout(timer);
  }, [transcribeQuery]);
  /** 「录音」列表的筛选词与排序键——与转写同一套口径、同一份控件（PRD-07-007）。 */
  const [recordingQuery, setRecordingQuery] = useState("");
  const [recordingSort, setRecordingSort] = useState<ListSortDto>(DEFAULT_RECORDING_SORT);
  const [recordingNeedle, setRecordingNeedle] = useState("");
  useEffect(() => {
    const timer = window.setTimeout(() => setRecordingNeedle(recordingQuery.trim()), 200);
    return () => window.clearTimeout(timer);
  }, [recordingQuery]);
  const [trashEntries, setTrashEntries] = useState<TrashEntryDto[]>([]);
  const [selectedPath, setSelectedPathRaw] = useState<string | null>(null);
  const setSelectedPath = useCallback((update: SetStateAction<string | null>) => {
    void withEditorSaveGuard(() => setSelectedPathRaw(update));
  }, []);
  const [status, setStatus] = useState<string>("");
  // 标签深链（tags://<tag> 导航或编辑器内联 #tag 点击；nonce 支持重复导航同一标签）。
  const [tagsFocus, setTagsFocus] = useState<{ tag: string; nonce: number } | null>(null);
  // 图谱局部聚焦深链（graph://<path>，编辑器「在图谱中查看」入口；nonce 支持重复导航）。
  const [graphFocus, setGraphFocus] = useState<{ path: string; nonce: number } | null>(null);
  // null = 加载中；false = 未选择 Vault（显示去引导/设置入口）；true = 已打开。
  const [vaultOpen, setVaultOpen] = useState<boolean | null>(initialVault?.open ?? null);
  /** 启动时配置指着、却打不开的库根（PRD-14-062）：空态据此说「找不到了」而不是「没选」。 */
  const [missingRoot, setMissingRoot] = useState<string | null>(
    initialVault?.missing_root ?? null,
  );
  /** 那个库不是找不到了，是 `.luna/` 还在 iCloud 上没取回：空态说「还在下载」、给「重试」。 */
  const [missingInCloud, setMissingInCloud] = useState(initialVault?.missing_in_cloud ?? false);

  /**
   * 库快照的版本号：任何"库变了"的信号都把它 +1。
   *
   * 三份分页列表把它编进各自的取数身份（`key`），故一次 `vault://changed` 就让它们各自
   * 从第一页重来——**重来是刻意的**：写盘之后已加载的那几页是不是还成立，前端答不了
   * （那条笔记可能改了名、换了类型、被删了），而拿旧游标去续一个已经变过的列表，正是
   * 键集游标要防的那种"看起来没错"的错。
   */
  const [vaultRevision, setVaultRevision] = useState(0);

  /**
   * 三份内容列表的分页取数（TDD-07 §4.1a）。
   *
   * **取数口径全在 `key` 里**：库版本 / 排序键 / 筛选词。它一变就丢掉游标从第一页重来
   * ——拿旧口径的游标去续新口径的列表，Rust 会当场判 `LIST_CURSOR_INVALID`。
   *
   * **`limit` 不给**：页大小是 Rust 的单一常量（`luna_vault::LIST_PAGE_SIZE`），前端
   * 连读都不需要读它。此前这里写着 `NOTE_LIST_LIMIT = 2000`——那就是本批要消掉的东西。
   */
  const vaultReady = vaultOpen === true;
  /** 创作空间的看板（ADR-162）：只在站在这一格时读——读看板要逐篇读项目主页。 */
  const creatorBoard = useCreatorBoard(isAt("creator") && vaultReady, vaultRevision);

  /**
   * 速记侧栏的筛选（PRD-06-058 A′）。
   *
   * **它进 `usePagedList` 的 key**：换一档筛选就是换一份清单，游标、已加载的页与
   * 勾选集都要从头来过。留在 key 之外的表现是"筛完还接着加载上一份的下一页"。
   */
  const [draftFilter, setDraftFilter] = useState<DraftFilterDto>("all");
  const [draftQuery, setDraftQuery] = useState("");
  // 输入每敲一个字都重取一次清单是白烧 IPC；这一份是**去抖之后**的值，只有它进 key。
  const draftQueryApplied = useDebouncedValue(draftQuery, 200);

  const draftList = usePagedList<DraftNoteDto>({
    key: `drafts|${vaultRevision}|${draftFilter}|${draftQueryApplied}`,
    enabled: vaultReady,
    fetchPage: async (cursor) => {
      const result = await commands.listDraftNotes(
        null,
        cursor,
        draftFilter,
        draftQueryApplied.trim() === "" ? null : draftQueryApplied,
      );
      if (result.status !== "ok") {
        return null;
      }
      // 信封上的两个计数与合并限额不随页变，取第一页时更新即可（后续页给的是同一份）。
      setDraftMeta({
        total: result.data.total,
        matched: result.data.matched,
        merge_min: result.data.merge_min,
        merge_max: result.data.merge_max,
      });
      return { items: result.data.notes, cursor: result.data.next_cursor };
    },
  });

  const recordingList = usePagedList<SearchHitDto>({
    key: `recordings|${vaultRevision}|${recordingSort}|${recordingNeedle}`,
    enabled: vaultReady,
    fetchPage: async (cursor) => {
      const result = await commands.search({
        text: null,
        note_type: "recording",
        tag: null,
        path_prefix: null,
        // 与转写清单同一口径：按标题/路径的 `name_like`，可分页可排序。全文检索的 `text`
        // 按相关度排序且不分页（TDD-07 §4.1a），与分页游标同传会被 Rust 直接拒绝——
        // 真机上表现为一输入筛选词整张清单就空掉。
        name_like: recordingNeedle || null,
        limit: null,
        page: { sort: recordingSort, cursor },
      });
      return result.status === "ok" ? pageOf(result.data) : null;
    },
  });

  const transcribeList = usePagedList<SearchHitDto>({
    key: `transcripts|${vaultRevision}|${transcribeSort}|${transcribeNeedle}`,
    enabled: vaultReady,
    fetchPage: async (cursor) => {
      const result = await commands.search({
        text: null,
        note_type: "transcript",
        tag: null,
        path_prefix: null,
        // 筛选词也下沉给 Rust：留在前端只会筛已加载的那几页，于是「筛出 0 条」与
        // 「库里没有」变成同一个画面，而后者是错的。
        name_like: transcribeNeedle || null,
        limit: null,
        page: { sort: transcribeSort, cursor },
      });
      return result.status === "ok" ? pageOf(result.data) : null;
    },
  });

  const recordingNotes = recordingList.items;
  const transcribeNotes = transcribeList.items;
  /**
   * 「转写」的主区空态问的是**库里有没有转写**，不是"当前筛选结果有没有"。
   *
   * 筛选下沉给 Rust 之后 `transcribeNotes` 是**筛过**的结果，只看它为空会让主区对着
   * 一个筛出 0 条的库说「你还没导入过任何东西」——侧栏那边已经如实说了「没有匹配」。
   */
  const transcribeLibraryHasNotes = transcribeNotes.length > 0 || transcribeNeedle.length > 0;

  /** 启动概览弹窗（仪表盘的新形态）：每次运行第一次进入工作区时呈现，关闭即笔记。 */
  const [dashboardOpen, setDashboardOpen] = useState(false);
  /** 概览底部那个勾选框的当前值：初值来自真实设置，用户改了当场跟手。 */
  const [showOnLaunch, setShowOnLaunch] = useState(launchOverview);
  const [quickSwitcherOpen, setQuickSwitcherOpen] = useState(false);
  const [commandPaletteOpen, setCommandPaletteOpen] = useState(false);
  const [templatePickerOpen, setTemplatePickerOpen] = useState(false);
  /** 主侧栏折叠（PRD-07-057 收起列表扩大编辑区；随布局快照持久化，跨视图记忆）。 */
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);
  const [panelCollapsed, setPanelCollapsed] = useState(true);
  /** 「添加内容」Popover（PRD-07-067 ②）：侧栏页头常驻，收起时同一动作退到顶栏。 */
  const [addContentOpen, setAddContentOpen] = useState(false);
  /** 主侧栏列宽（px；null = 未定制走 CSS 默认；全视图共享，随布局快照持久化）。 */
  const [sidebarWidth, setSidebarWidth] = useState<number | null>(null);
  /** 语音速记视图单编辑器的字数/脏态（笔记页走 paneMetrics）。 */
  const [editorMetrics, setEditorMetrics] = useState<EditorMetrics>(EMPTY_EDITOR_METRICS);
  /** 笔记主区标签页 + 分屏布局（PRD-07-035/036；状态机在 note-panes.ts）。 */
  const [panesState, setPanesStateRaw] = useState<NotePanesState>(initialNotePanesState);
  // 命令上下文是 memo 化的：要知道「此刻有没有可重开的标签」得经 ref 读。
  const panesStateRef = useRef(panesState);
  panesStateRef.current = panesState;
  // 预览标签（PRD-07-035 2026-10-05 修订）：浏览类入口以预览打开、明确意图以常驻打开；
  // 开关关掉时两者都退回「复用当前标签」（`as` 缺省）。
  const previewTabs = useBoolSetting("editor.preview_tabs", true);
  const browseAs = previewTabs ? ("preview" as const) : undefined;
  const intentAs = previewTabs ? ("permanent" as const) : undefined;
  /** 改名 / 移动的回执与撤销（PRD-06-030 ③）：入口各处，宿主只有这一个。 */
  const [relocationReceipt, dismissRelocationReceipt] = useRelocationReceipt();
  const externalRenamePrompt = useExternalRenamePrompt();
  const [obsidianSuggestion, settleObsidianSuggestion] = useObsidianSuggestion();
  const setPanesState = useCallback((update: SetStateAction<NotePanesState>) => {
    void withEditorSaveGuard(() => setPanesStateRaw(update));
  }, []);
  /** 每个 pane 当前编辑器的字数/脏态（标签脏点 + 状态栏取活跃 pane）。 */
  const [paneMetrics, setPaneMetrics] = useState<
    Record<number, EditorMetrics>
  >({});
  /** 布局快照已从 Vault 恢复（恢复前不写盘，避免初始空布局覆盖上次会话）。 */
  const [layoutRestored, setLayoutRestored] = useState(false);
  /**
   * 界面上要展示的那几条全局键（原始注册表值；展示前过 `formatShortcut`）。
   *
   * 一张表而不是四个 `useState`：它们的取数、订阅与"没绑定就是空串"逐字相同，各写一份的
   * 结果是加第五条时**只加了 state、忘了加取数**——那颗按钮于是永远不显示快捷键。
   * 界面不写第二份默认值，一律取 Rust 下发的真值。
   */
  const { bindings: shortcutHints, doubleTapWindowMs } = useShortcutBindings(SHORTCUT_HINT_KEYS);
  const doubleTapDetector = useRef(new DoubleTapDetector());
  const recordingShortcut = shortcutHints["shortcuts.recording_toggle"] ?? "";
  const voiceInputShortcut = shortcutHints["shortcuts.voice_input"] ?? "";
  const textQuickNoteShortcut = shortcutHints["shortcuts.text_quick_note"] ?? "";
  const quickChatShortcut = shortcutHints["shortcuts.quick_chat"] ?? "";
  /* 删除策略与它派生的那句「删了之后会怎样」——回收站页按前者选形态，全部删除确认卡按
     后者说话（此前逐处硬写「可从回收站还原」，默认策略下指错地方、永久删除档下是反的）。 */
  const deleteMode = useDeleteMode();
  const deleteConsequence = useDeleteConsequence();
  // 译境实验室入口的可用性与运行态：启用后才显示顶栏图标，快捷键共用同一运行态。
  const desktopLens = useDesktopLens();
  const [selectedRecordingPath, setSelectedRecordingPath] = useState<string | null>(null);
  /** 创作空间主区此刻在哪（ADR-165 ⑥）：看板 / 灵感库 / 某个项目的某个页签 / 一篇灵感。 */
  const [creatorView, setCreatorView] = useState<CreatorView>({ view: "board" });
  /** 新建项目对话框（开着时的状态；`null` = 关着）。 */
  const [newProjectDialog, setNewProjectDialog] = useState<{ busy: boolean; error: string | null } | null>(null);
  /** 创作空间主区里此刻在编辑的那一篇（上下文面板、状态栏据此跟上）；没有编辑器时为 `null`。 */
  const [creatorDocumentPath, setCreatorDocumentPath] = useState<string | null>(null);
  /** 「应用」目的地此刻看的是哪个应用的详情（ADR-165 ⑤）；`null` = 列表。 */
  const [appsSelection, setAppsSelection] = useState<string | null>(null);
  const firstPartyApps = useApps();
  /** 「从链接导入」应用开着没有：入口随它出现与收回（ADR-167 ⑤）。 */
  const linkImportEnabled = firstPartyApps.some((app) => app.id === ("link_import" satisfies AppId) && app.enabled);
  /** 此刻到不了的空间 → 它所属的应用 id（应用未启用，ADR-165 ③ ④）。清单取回前为空。 */
  const appOfUnavailableSpace = useMemo(() => {
    const map = new Map<WorkspaceSpace, string>();
    for (const app of firstPartyApps) {
      if (!app.enabled && app.space) map.set(app.space, app.id);
    }
    return map;
  }, [firstPartyApps]);
  const unavailableSpaces = useMemo(() => new Set(appOfUnavailableSpace.keys()), [appOfUnavailableSpace]);
  const appOfUnavailableSpaceRef = useRef(appOfUnavailableSpace);
  appOfUnavailableSpaceRef.current = appOfUnavailableSpace;
  // 站在一个所属应用已停用的空间上（停用那一刻、或深链在清单取回前把人带了进来）：改送到
  // 那个应用的详情页——它说明这里为什么没了、怎么再打开（ADR-165 ④）。
  useEffect(() => {
    const owner = appOfUnavailableSpace.get(location.space);
    if (owner) {
      setAppsSelection(owner);
      setLocation(locationFor("apps"));
    }
  }, [appOfUnavailableSpace, location.space]);
  /**
   * 录音清单的多选（PRD-06-059）与它的可见行。
   *
   * **选择集与"正在回放的那一篇"同源但不同义**：`selectedRecordingPath` 答的是右侧
   * 工作面在放谁（`aria-current`），选择集答的是批量动作作用在谁身上（`aria-selected`）。
   * 可见行由清单上报——收起的文件夹里那些行不在其中（看不见的不算选中）。
   */
  const [recordingRows, setRecordingRows] = useState<readonly string[]>([]);
  const recordingSelection = useNoteSelection({
    current: selectedRecordingPath,
    visible: recordingRows,
    // Delete 键 = 工具栏那颗删除按钮（同一条通道、同一张确认卡）。
    onDelete: () => deleteRecordingSelection(),
  });
  /**
   * 状态栏「录音中 · 计时」（B2，PRD-07-045 / PRD-04-003）：以 Rust 录音控制器的状态转移
   * 广播为准。主窗不是录音的发起窗——它此前听的 `luna:recording-started` 自定义事件没有
   * 任何地方派发过，于是麦克风在录、文件在长，状态栏却一直写着「就绪」。
   */
  const [recordingIndicator, setRecordingIndicator] = useState<RecordingIndicator | null>(null);
  /**
   * 统一处理队列（导入/二次转写/二次翻译/说话人细分，PRD-04-032/055）在根上**只取一位**：
   * 启动概览「有事才弹」要的「快照到没到、有没有在跑的」。整份队列与进度归顶栏入口、回放
   * 视图与清单行各自按需订（2026-10-15 真机卡顿：根持有整份队列 = 每一拍重渲染整个工作区）。
   */
  const launchQueue = useProcessingQueueSelector((state) =>
    !state.loaded ? "loading" : activeCount(state.items) > 0 ? "busy" : "idle",
  );
  const [queueOpen, setQueueOpen] = useState(false);
  /**
   * 「现在什么在处理」这条状态路径的观测（PRD-07-070 / TDD-12 §2.1）。
   *
   * **宿主是持有 `queueOpen` 的这一层，不是面板也不是顶栏入口**：到访的终点是面板关上
   * 那一刻，而面板条件渲染、关闭即卸载，自己报不了这一刻；挂在入口按钮上则要在三条关闭
   * 路径（关闭钮、`setQueueOpen(false)`、跳转笔记后收起）上各记一次，那正是"重复各报一
   * 次"。三条路径只在这个开关上汇合，所以观测生在这里、句柄传给面板。
   */
  const queueVisit = useStatusVisitMetrics("queue", queueOpen);
  /** 「从链接导入」对话框（PRD-05-006）：转写侧栏与主区 landing 共用同一实例。 */
  const [urlImportOpen, setUrlImportOpen] = useState(false);
  const [autoFocusTitle, setAutoFocusTitle] = useState(false);
  /**
   * 刚转正的那一篇（撤销条的对象）；`null` = 不显示。
   *
   * **住在宿主而不是侧栏**（2026-09-09 评审）：转正有两个入口——侧栏行上那枚 ✓ 与主区
   * 「整理」栏那颗按钮，而撤销条此前只长在侧栏里，于是**从主区转正的人根本没有撤销**。
   * 状态上移之后两个入口共用同一条，且它出现在转正后空出来的主区里——用户视线所在处。
   */
  const [undoSettlePath, setUndoSettlePath] = useState<string | null>(null);
  // 撤销条挂 8 秒：转正是一步可逆动作，安全感值这几秒，但它不该赖着不走。
  useEffect(() => {
    if (undoSettlePath === null) {
      return;
    }
    const timer = window.setTimeout(() => setUndoSettlePath(null), UNDO_SETTLE_WINDOW_MS);
    return () => window.clearTimeout(timer);
  }, [undoSettlePath]);
  /** 共享 AI 对话控制器（主区对话 ChatView 与主侧栏历史列表共用同一会话，PRD-03-013）。 */
  const chatController = useChatController("workspace");
  /**
   * 根上只取会话的这一位：「当前链接的是哪一篇」（侧栏高亮、多选、上下文面板都认它）。
   * 消息列表由对话视图自己订——流式回答一个 token 一换，挂在根上就是整个工作区一拍一渲（D1）。
   */
  const chatNotePath = useChatSessionSelector(chatController.store, (session) => session.notePath);
  /** Vault 检索控制器（PRD-06-031）：查询与结果随目的地切换保留，回来还是刚才那一屏。 */
  const searchController = useVaultSearch();
  /** 图谱控制器（PRD-06-034/035）：侧栏控制面与主区画布共享单一实例；数据加载随目的地激活。 */
  const graphController = useGraphController(isAt("knowledge", "network"));
  /** 右键菜单宿主与共享的笔记条目集（PRD-07-059 / PRD-07-004）。 */
  const contextMenu = useContextMenu();
  const ask = useConfirm();
  const noteMenu = useNoteMenu();
  // 批量整理知识笔记（PRD-06-044）：唯一常驻实例，确认卡与进度条共享它。
  const distillBatch = useDistillBatch();
  // 重生成差异预览（PRD-06-045）：同一常驻宿主模式，落盘后直接打开产出。
  const distillPreview = useDistillPreview({ onApplied: (path) => openNoteInPanes(path) });
  // 多库切换（PRD-06-049）：护栏判定在 Rust，本控制器只管三态呈现与确认。
  const [vaultManagerOpen, setVaultManagerOpen] = useState(false);
  const vaultSwitch = useVaultSwitch({
    // 换库 = 换掉整个工作区的内容源：宿主（App）以 key 整体**重挂载**本组件——
    // 布局按新库 `.luna/workspace.json` 重新恢复（PRD-07-064），标签页/对话/搜索结果/
    // 图谱等一切前端态一次归零。逐个视图手工清理必然漏掉下一个新增的视图态。
    onSwitched: () => onVaultSwitched?.(),
  });
  const prompt = usePrompt();
  // 外部 `lunarca://` 库内容链接（ADR-141，W4a）：判定全在 Rust，这里只提供各动作的落点。
  useVaultLinks({
    confirmSwitch: (vaultName) =>
      ask({
        title: t("vaultLink.switch_title", { name: vaultName }),
        description: t("vaultLink.switch_desc", { name: vaultName }),
        confirmLabel: t("vaultLink.switch_confirm"),
      }),
    switchVault: (path, name) => vaultSwitch.request(path, name),
    open: (path, line, headingIndex) => {
      openNoteInPanes(path);
      if (line !== null) {
        requestEditorReveal({ path, line, headingIndex, focusEditor: true });
      }
    },
    search: (query) => {
      searchController.setQuery(query);
      go("search");
    },
    // 写须确认（ADR-141 ③）：外部只给了一个名字，建不建由用户在这张卡上说了算。
    createNamed: async (name) => {
      const confirmed = await prompt({
        title: t("vaultLink.new_title"),
        description: t("vaultLink.new_desc"),
        label: t("vaultLink.new_label"),
        value: name,
        confirmLabel: t("vaultLink.new_confirm"),
      });
      if (confirmed === null) {
        return;
      }
      const result = await commands.createNoteNamed(confirmed);
      if (result.status === "ok") {
        openNoteInPanes(result.data.path);
      } else {
        setStatus(vaultErrorText(result.error));
      }
    },
    report: (reason) => setStatus(t(`vaultLink.rejected.${reason}`)),
  });
  /** 媒体导入动作（「转写」目的地的主动作，PRD-05-030）。 */
  const transcribeImport = useTranscribeImport(() => setStatus(t("status.error")));
  // 「添加内容 ▸ 导入资料」（ADR-164 ⑩）：音视频与文档同一个选择框，分流在 Rust；「转写」页自己的
  // 导入按钮仍只认音视频（上面那个）。
  const materialImport = useTranscribeImport(
    () => setStatus(t("status.error")),
    commands.importMaterialPaths,
    commands.pickMaterialFiles,
  );
  /** 从系统拖入音/视频即入队（PRD-05-001）：窗口级监听，落到哪个目的地都受理。 */
  const mediaDrop = useMediaDrop(
    () => {
      setQueueOpen(true);
      go("transcripts");
    },
    () => setStatus(t("status.error")),
  );

  // 活动栏布局（PRD-07-003）：顺序、显隐、锚点由 Rust 归一化后下发；录音进行中「录音」
  // 那格无论是否收起都要在场——正在录的东西不能从导航上消失。
  const railLayout = useRailLayout();
  const railForceVisible = useMemo(
    () => new Set<WorkspaceSpace>(recordingIndicator ? ["recordings"] : []),
    [recordingIndicator],
  );

  // 主侧栏可见 = 该空间有列表 且 未被用户收起（PRD-07-057 自动隐藏 + 手动收起）。
  const sidebarVisible = hasSidebar(space) && !sidebarCollapsed;

  // **折叠是随一级空间的临时态**（PRD-07-057 ④）：`space` 变化即复位展开，不持久化。
  // 依赖刻意只有 `space`——切二级分区时侧栏本就在显示那个空间的列表，把它重新展开等于
  // 撤销用户刚刚做的"腾地方"动作。再点当前活动图标的 toggle 不改 `space`、不触发本 effect。
  useEffect(() => {
    setSidebarCollapsed(false);
  }, [space]);

  /** 打开标签视图并选中指定标签（编辑器内联 #tag 点击 / tags:// 深链共用）。 */
  function openTagView(tag: string) {
    setTagsFocus((current) => ({ tag, nonce: (current?.nonce ?? 0) + 1 }));
    tagController.selectTag(tag);
    go("knowledge", "tags");
  }

  /** 标签页主区与侧栏右键共用同一搜索路由。 */
  function openTagSearch(tag: string) {
    searchController.setQuery(`tag:${tag}`);
    go("search");
  }

  /** 打开一篇知识产物（ADR-096 ③）：落「知识库 → 主题页」并选中它——不进编辑桌。 */
  function openKnowledgeProduct(path: string) {
    setTopicSelection(path);
    go("knowledge", "topics");
  }

  /** 产物闸口（ADR-096 ③）：是产物就让路到知识库并返回 `true`。**每一条**会把路径
      送上编辑桌的通路都必须先过它——真机第六轮坐实：搜索空间与编辑器内链两条
      「共用编辑桌、不切空间」的通路绕开了 `openNoteInPanes`，主题页被打开成可编辑。 */
  function routeIfKnowledgeProduct(path: string): boolean {
    if (!productPaths.has(path)) {
      return false;
    }
    openKnowledgeProduct(path);
    return true;
  }

  /** 在笔记页打开路径（文件或文件夹）：多标签语义统一入口（PRD-07-035）。
      **编辑桌对知识产物拒开并转发**（ADR-096 ③ / PRD-07-058 护栏）：搜索命中、
      快速切换器、图谱节点、引用芯片……所有直开路径都汇到这里，闸口只设一处。 */
  function openNoteInPanes(path: string, options?: OpenNoteOptions) {
    if (routeIfKnowledgeProduct(path)) {
      return;
    }
    setPanesState((state) => openNote(state, path, { as: intentAs, ...options }));
    go("notes");
  }

  /** 在右侧新分屏打开（文件树右键、快速切换器 ⌘Enter 共用，PRD-07-036 / PRD-07-040 ③）。 */
  function openNoteInSplit(path: string) {
    if (routeIfKnowledgeProduct(path)) {
      return;
    }
    setPanesState((state) => {
      const split = splitPane(state, state.activePaneId);
      return openNote(split, path, { paneId: split.activePaneId });
    });
  }

  /** 在独立窗口打开一篇笔记（PRD-07-036 弹出窗口）。`move` = 同时从主区标签移出。 */
  async function openNoteWindow(
    path: string,
    options?: { move?: { paneId: number; tabId: number } },
  ) {
    // 产物同样不进独立编辑窗口（与主窗编辑桌同一条护栏）。
    if (routeIfKnowledgeProduct(path)) {
      return;
    }
    const result = await commands.openNoteWindow(path);
    if (result.status === "error") {
      setStatus(t("status.error"));
      return;
    }
    if (options?.move) {
      const { paneId, tabId } = options.move;
      // 移到独立窗口不是关闭：不进 ⌘⇧T 的重开栈。
      setPanesState((state) => removeTab(state, paneId, tabId));
    }
  }

  // 笔记页的「当前选择」派生自活跃 pane 的活跃标签（面包屑/上下文面板/文件树高亮同源）。
  useEffect(() => {
    if (location.space === "notes") {
      setSelectedPathRaw(activeNoteTab(panesState)?.path ?? null);
    }
    // pane 被状态机收敛移除（关掉最后一个标签等）时回收其字数/脏态残留。
    setPaneMetrics((current) => {
      const alive = new Set(panesState.panes.map((pane) => pane.id));
      const staleIds = Object.keys(current).filter((id) => !alive.has(Number(id)));
      if (staleIds.length === 0) {
        return current;
      }
      const next = { ...current };
      for (const id of staleIds) {
        delete next[Number(id)];
      }
      return next;
    });
  }, [panesState, location]);

  // 启动概览弹窗（PRD-07-050 / PRD-14-068「有事才弹」）：每次运行、工作区首次就绪时至多
  // 呈现一次，且只在处理队列有活或装机仍有未完成项时才弹。首启引导期间 vaultOpen 非 true
  // 故不弹；切库重挂载不再弹（module 级标志跨挂载存活）；关闭即露出下面的笔记视图。
  // 受 `appearance.launch_overview` 总闸门控（真机诉求「可选择下次不弹」）——启动快照里
  // 读一次即可：本次运行弹不弹在此刻已定，设置页里改的是下一次。
  // 装机就绪状态（PRD-14-063）：横幅与启动概览门控共用这一份。
  const { setup: setupStatus } = useSetupStatusContext();
  // 「有事才弹」（PRD-14-068）：处理队列有活、或装机仍有未完成项时才弹启动概览；
  // 什么都没有就不打扰——想看时从命令面板「打开概览」叫出来（`open_overview` 命令）。
  // **两份数据都到齐才判**：判据锁死是一次性的，谁没到就把"还不知道"当成了"没事"。
  // 队列快照是另一条订阅，此前只等 `setup_status`——冷启动时队列多半还是空数组，
  // 于是"有任务在跑"这个触发条件基本不会成立。
  useEffect(() => {
    if (
      vaultOpen !== true ||
      dashboardShownThisRun ||
      setupStatus === null ||
      launchQueue === "loading"
    ) {
      return;
    }
    dashboardShownThisRun = true;
    const somethingToShow =
      launchQueue === "busy" || projectSetupBanner(setupStatus).firstRemaining !== null;
    if (launchOverview && somethingToShow) {
      setDashboardOpen(true);
    }
  }, [vaultOpen, launchOverview, setupStatus, launchQueue]);

  // 弹窗内点任意深链卡片（luna:navigate）即自行关闭——用户的意图是"去那儿"，
  // 弹窗完成了引导使命就该让路。
  // 启动概览弹窗：任意导航即关（深链会把用户带走，弹窗留在原地就成了挡路的）。
  // Esc / 焦点圈定 / 焦点归还归共享 hook（见下方 `overviewRef`）——此处只剩导航这一条。
  useEffect(() => {
    if (!dashboardOpen) {
      return;
    }
    const close = () => setDashboardOpen(false);
    window.addEventListener(NAVIGATE_EVENT, close);
    return () => window.removeEventListener(NAVIGATE_EVENT, close);
  }, [dashboardOpen]);

  const overviewRef = useDialogBehavior<HTMLDivElement>({
    onClose: () => setDashboardOpen(false),
    enabled: dashboardOpen,
  });

  // 布局快照恢复（PRD-07-035 重启恢复）：Vault 打开后一次性加载，Rust 侧已剪枝失效路径。
  useEffect(() => {
    if (vaultOpen !== true) {
      return;
    }
    let disposed = false;
    void commands.workspaceLayout().then((result) => {
      if (disposed) {
        return;
      }
      if (result.status === "ok" && result.data) {
        setPanesState(restoreNotePanes(result.data));
        const width = result.data.sidebar_width ?? 0;
        setSidebarWidth(width > 0 ? width : null);
      }
      setLayoutRestored(true);
    });
    return () => {
      disposed = true;
    };
  }, [vaultOpen]);

  // 布局快照持久化（防抖）：恢复完成后布局每次变化延迟写回 `.luna/workspace.json`。
  // 切库在途时暂停——Rust 换根发生在防抖窗口内的话，600ms 定时器会把**旧库**的
  // 标签页写进**新库**的 workspace.json（跨库布局污染）；切换完成即整体重挂载，
  // 新实例从新库的快照重新出发。
  useEffect(() => {
    if (vaultOpen !== true || !layoutRestored || vaultSwitch.switchingTo !== null) {
      return;
    }
    const timer = window.setTimeout(() => {
      const panes = serializeNotePanes(panesState) ?? { panes: [], active_pane: 0 };
      void commands.setWorkspaceLayout({
        ...panes,
        sidebar_width: sidebarWidth ?? 0,
      });
    }, 600);
    return () => {
      window.clearTimeout(timer);
    };
  }, [panesState, vaultOpen, layoutRestored, sidebarWidth, vaultSwitch.switchingTo]);

  // 最近打开（PRD-07-040 ②，设备状态）：活跃标签换成一个文件就记一次；文件夹卡片、新标签
  // 页由 Rust 忽略。切库在途时不记——那一刻的路径属于旧库。
  const activeTabPath = activeNoteTab(panesState)?.path ?? null;
  // 命令上下文只建一次：「在提词器中打开这篇」经 ref 取此刻那一篇。
  const activeTabPathRef = useRef(activeTabPath);
  activeTabPathRef.current = activeTabPath;
  useEffect(() => {
    if (vaultOpen !== true || vaultSwitch.switchingTo !== null || !activeTabPath) {
      return;
    }
    void commands.recordRecentNote(activeTabPath);
  }, [activeTabPath, vaultOpen, vaultSwitch.switchingTo]);

  // ── 主侧栏拖拽调宽（PRD-07-057）────────────────────────────────────────────
  // clamp 区间与 Rust `SIDEBAR_WIDTH_MIN/MAX`（加载端权威剪枝）同口径；起始宽度读侧栏
  // 实际渲染宽（未定制时即 CSS 默认，不在 TS 复写默认值）；拖拽中只更新本地宽度，
  // 持久化由布局快照防抖 effect 承担。
  function startSidebarResize(event: React.MouseEvent) {
    event.preventDefault();
    const startX = event.clientX;
    const sidebarEl = event.currentTarget.parentElement as HTMLElement | null;
    const startWidth = sidebarWidth ?? sidebarEl?.offsetWidth ?? SIDEBAR_WIDTH_MIN;
    const onMove = (ev: MouseEvent) => {
      setSidebarWidth(
        Math.min(SIDEBAR_WIDTH_MAX, Math.max(SIDEBAR_WIDTH_MIN, startWidth + ev.clientX - startX)),
      );
    };
    const onUp = () => {
      window.removeEventListener("mousemove", onMove);
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp, { once: true });
  }

  /**
   * 主区是不是一块**可写画布**——多标签编辑面（全部笔记 / 搜索共用那张桌子）∪ 速记的
   * 单编辑器。字数/保存态与「还没选东西」的面包屑话术都只对画布成立。
   *
   * 判据取 `hasEditorSurface(location)` 而不是 `space === "notes"`：**搜索与笔记共用
   * 同一片编辑面**（PRD-07-058），按空间判会让搜索里打开的那篇笔记既不报字数、也拿不到
   * 上下文面板——而它明明就在主区上摊着。
   */
  // 创作空间里有编辑器时（项目的概览 / 研究 / 脚本页签、一篇灵感）同样是一张写作的桌子：
  // 上下文面板与状态栏的字数、口播时长跟着它走（ADR-165 ⑥）。
  const editorCanvasVisible =
    hasEditorSurface(location) || isAt("quick_notes") || (isAt("creator") && creatorDocumentPath !== null);
  const activePaneId = activePaneOf(panesState).id;
  const activeMetrics = hasEditorSurface(location)
    ? (paneMetrics[activePaneId] ?? EMPTY_EDITOR_METRICS)
    : editorMetrics;
  const tabFocusMode = useTabFocusMode();
  const selectedDraftPath =
    isAt("quick_notes") && draftList.items.some((note) => note.path === selectedPath)
      ? selectedPath
      : null;

  /**
   * 一级角标：只有「速记」有——未处理速记数（PRD-07-066）。
   *
   * **通知未读不在这里**：它只挂状态栏铃铛（ADR-087 ②）。两处角标各答各的问题，
   * 位置与取数口径都不共用。
   */
  const draftBadge: NavBadge = {
    count: draftMeta.total,
    labelKey: "draft.navPending",
    numeric: true,
  };
  const railBadges: Partial<Record<WorkspaceSpace, NavBadge>> = { quick_notes: draftBadge };

  /** 二级角标：知识库三个分区此刻都没有需要挂的计数（PRD-07-066 的计数只在一级）。 */
  const sectionBadges: Partial<Record<WorkspaceSection, NavBadge>> = {};

  /**
   * 当前正在查看的对象路径——「我在看什么」的**唯一取数口径**（PRD-07-057）。
   *
   * 顶栏面包屑是它的唯一呈现处：状态栏此前又原样重复了一遍同一路径（还因宽度被截断成
   * `Recordings/录音_20260724_13…`），两处同义信息只会分散注意力（真机反馈）。
   */
  const activeDocumentPath = hasEditorSurface(location)
    ? // 编辑面上"当前那一篇"是**活跃 pane 的活跃标签**，不是文件树的选择：搜索里根本
      // 不同步树选择，而分屏时两者也未必是同一篇。
      (activeNoteTab(panesState)?.path ?? null)
    : isAt("quick_notes")
      ? selectedDraftPath
      : isAt("recordings")
        ? selectedRecordingPath
        : isAt("transcripts")
          ? selectedTranscribePath
          : isAt("chat")
            ? chatNotePath
            : isAt("creator")
              ? creatorDocumentPath
              : null;

  /**
   * 「上下文」面板可用吗——判据是**当前主区确有一篇 Markdown 笔记**（PRD-07-063），
   * 不再是一张易漂移的目的地白名单。
   *
   * 旧判据同时错在两头：设置/回收站之外它在「标签」这种**集合页**上也会出现（点开只
   * 得到一句「未选择」），而在录音/转写上它描述的又是 `selectedPath`——那是笔记空间的
   * 选择，与主区正在放的那条录音无关。收敛成一份投影后，两个毛病同时没了。
   */
  const contextNotePath = activeDocumentPath?.endsWith(".md") ? activeDocumentPath : null;
  const contextPanelAvailable = contextNotePath !== null;

  /**
   * 面包屑分段。**末段去掉 `.md`**：文件树、标签页与编辑器标题都按笔记名呈现（不带
   * 扩展名），只有这里把路径原样劈开——同一篇笔记在同屏四处出现，唯独面包屑多一个
   * `.md`，读者会以为那是另一个东西。扩展名是存储细节，不是笔记的名字。
   * 非 md（附件/文件夹）保持原样：那时扩展名**就是**它的身份的一部分。
   */
  const breadcrumbs = activeDocumentPath
    ? activeDocumentPath
        .split("/")
        .map((segment, idx, all) =>
          idx === all.length - 1 ? segment.replace(/\.md$/u, "") : segment,
        )
    : editorCanvasVisible
      ? // 面包屑说的是"你在这一格里还没选东西"，故话术跟着目的地走（PRD-06-058 P1-1）：
        // 站在「记录」里被告知「未选择笔记」，说的是另一格的事。
        [t(`canvas.noSelection.${canvasDestination(location)}`)]
      : [t(spaceLabelKey(space))];

  /** 文件夹占位卡统计（按 pane 的活跃标签路径取，分屏各自独立）。 */
  const folderStatsFor = useMemo(() => {
    return (path: string | null): { files: number; folders: number } | null => {
      if (!path || path.endsWith(".md") || !tree) {
        return null;
      }
      const node = findNode(tree, path);
      return node ? countFolderChildren(node) : null;
    };
  }, [tree]);

  /** 库还在 iCloud 上时的「重试」：照原路径再开一次；Rust 侧还没取回就再说一遍「还在下载」。 */
  async function retryVaultInCloud(root: string) {
    const result = await commands.selectVault(root);
    if (result.status === "ok") {
      onVaultSwitched?.();
    } else {
      setStatus(vaultErrorText(result.error));
    }
  }

  /** 「上次的库找不到了」→ 让用户指回它现在的位置（PRD-14-062）。 */
  async function relocateVault() {
    const picked = await commands.pickFolder(null);
    if (picked.status !== "ok" || !picked.data) {
      return;
    }
    const result = await commands.selectVault(picked.data);
    if (result.status === "ok") {
      // 绑定成功 = 一次切库：整体重挂载按新库快照恢复布局。
      onVaultSwitched?.();
    } else {
      setStatus(vaultErrorText(result.error));
    }
  }

  async function refreshVault() {
    setStatus(t("status.loading"));
    // 先把版本推上去：三份列表据此各自重取第一页（它们不在这条 await 链上，各自有加载态）。
    setVaultRevision((revision) => revision + 1);
    const [treeResult, trashResult, productResult] = await Promise.all([
      commands.vaultTree(),
      commands.listTrash(),
      commands.knowledgeProductPaths(),
    ]);

    if (treeResult.status === "ok") {
      setTree(treeResult.data);
    } else {
      setStatus(t("status.error"));
      return;
    }
    if (productResult.status === "ok") {
      const products: ReadonlySet<string> = new Set(productResult.data);
      setProductPaths(products);
      productPathsRef.current = products;
    }

    setTrashEntries(trashResult);
    setStatus(t("status.ready"));
    // 冷启动里程碑「工作区就绪」（TDD-13 §6）：Rust 侧幂等，切库重取不会再记一次。
    void commands.startupMark("workspace_ready");
  }

  useEffect(() => {
    if (!status) {
      setStatus(t("status.loading"));
    }
  }, [t]);

  // 库变了就重取整份工作区数据（树 + 四条清单 + 回收站）。
  // **合并同一阵变更**（`use-vault-reload`）：一次批量移动/删除会逐条广播，逐条各拉
  // 一遍全量数据会把 Vault 锁排满——真机上"拖完之后界面不动"有一半是这么放大出来的。
  useVaultReload(() => void refreshVault());

  useEffect(() => {
    let disposed = false;

    async function init() {
      // 首次挂载吃启动快照（PRD-14-066）；切库重挂载时快照已过期，重读一次。
      const vaultStatus = initialVault ?? (await commands.vaultStatus());
      if (disposed) {
        return;
      }
      setVaultOpen(vaultStatus.open);
      setMissingRoot(vaultStatus.missing_root);
      setMissingInCloud(vaultStatus.missing_in_cloud);
      if (vaultStatus.open) {
        void refreshVault();
      }
    }
    void init();

    // 后端发起的导航（辅助窗口经 open_workspace_route 唤起、连接链接送达）：请求存在 Rust，
    // 监听挂上之后取一次（冷启动时它比工作区先到，事件早已发完），之后每收到一次事件再取一次。
    // 取即清，所以无论事件与这次挂载谁先到，同一次请求只落地一次。**先挂监听再取**：反过来
    // 的话，取完、监听挂上之前存进来的那一次既取不到也收不到事件，要等到下一次导航才落地。
    const takePendingNavigation = async () => {
      const route = await commands.takePendingNavigation();
      // **不看 `disposed`**：取走即已从 Rust 清掉，这里再丢就真的没了。StrictMode 的
      // 挂载-卸载-再挂载正是如此——第一次挂载取到、第二次取空；派发给 window 时接手的是
      // 此刻挂着的那个工作区的处理器。
      if (route === null) {
        return;
      }
      // 有确切去处的启动不再弹启动概览：概览是「没地方要去」时的落点，这一跳刚把用户
      // 带到他点链接要去的地方，再压一张概览上去，看起来就像没跳过去。
      dashboardShownThisRun = true;
      // 这里**直接派发**而不是调 `goTo`：本处理器是"跨窗口请求落地"的那一端，
      // 再走一次 `goTo` 就有把它原样送回 Rust 的可能（无人受理的 route 正是如此），
      // 两端互相转发就是一个死循环。
      window.dispatchEvent(new CustomEvent(NAVIGATE_EVENT, { detail: route }));
    };
    const unlistenNavigate = events.workspaceNavigate.listen(() => void takePendingNavigation());
    void unlistenNavigate.then(() => takePendingNavigation());
    const unlistenSettings = events.settingsChanged.listen((event) => {
      // 「显示隐藏文件」（V4b）改变的是文件树的内容，Rust 那边已按新值建树，重取即可。
      if (event.payload.key === "vault.show_hidden_files") {
        void refreshVault();
      }
    });
    const unlistenScan = events.scanProgress.listen((event) => {
      const progress = event.payload;
      // 进度文案由 `useIndexStatus` 驱动；这里只管终态刷新（取消的那一轮已写进的批次同样有效）。
      if (progress.done) {
        setVaultOpen(true);
        void refreshVault();
      }
    });

    return () => {
      disposed = true;
      void unlistenNavigate.then((dispose) => {
        dispose();
      });
      void unlistenScan.then((dispose) => {
        dispose();
      });
      void unlistenSettings.then((dispose) => {
        dispose();
      });
    };
  }, [t]);

  useEffect(() => {
    const onNavigate = (event: Event) => {
      const route = event instanceof CustomEvent ? String(event.detail) : "";
      // **"这条前缀落在哪一格"归 `parseWorkspaceRoute` 一处判定**（纯函数、可直接
      // 断言落点）；本处理器只负责把那个意图执行掉。未知 scheme 解析为 `null` = 无人
      // 受理，不做任何静默重定向（ADR-046）。
      const intent = parseWorkspaceRoute(route);
      if (intent === null) {
        return;
      }
      // 这一格接下了：`goTo` 据此不再把它兜一圈 Rust（辅助窗里没有本处理器，正是靠
      // 这个差别自动走「唤起主窗」那条路）。
      navigationHandled(event);
      switch (intent.kind) {
        case "settings":
          // 设置深链（settings://ai_services 等）：切到归属页面并滚动定位到目标。
          openSettingsRoute(intent.route);
          break;
        case "checkup":
          // 知识体检深链（体检完成/周期提醒通知）→ 打开报告面板（常驻宿主，PRD-16-032）。
          requestKnowledgeCheckup();
          break;
        case "queue":
          // 队列/任务进度深链 → 顶栏处理队列面板（任务态的常驻真身；仪表盘已弹窗化，
          // 不再是可落地的目的地，PRD-07-057 二次修订）。
          setQueueOpen(true);
          break;
        case "goto":
          setLocation(intent.location);
          break;
        case "tag":
          if (intent.tag) {
            openTagView(intent.tag);
          } else {
            setLocation(intent.location);
          }
          break;
        case "graph":
          if (intent.path) {
            const path = intent.path;
            setGraphFocus((current) => ({ path, nonce: (current?.nonce ?? 0) + 1 }));
          }
          setLocation(intent.location);
          break;
        case "recording":
        case "transcript":
          // 录音 / 转写深链带对象时**先选中再切格**：这两格的主区是"选中哪一条就放哪一条"，
          // 只切格不选中，用户看到的是一张列表加一句「选择一个录音以回放」——而他刚刚
          // 点的明明是某一篇上的入口。两个 setter 各归各的空间，不共用一个"当前对象"。
          // 历史通知兜底（真机第六轮）：早期主题合成通知存的是 `recording://<产物>`
          // （当时 note_route 对非转写一律按录音兜底），存量行改不了——按产物清单让路。
          if (intent.path && productPathsRef.current.has(intent.path)) {
            setTopicSelection(intent.path);
            setLocation(locationFor("knowledge", "topics"));
            break;
          }
          if (intent.path) {
            if (intent.kind === "recording") {
              setSelectedRecordingPath(intent.path);
            } else {
              setSelectedTranscribePath(intent.path);
            }
          }
          setLocation(intent.location);
          break;
        case "quick_note":
          // 速记格的主区是"选中哪一条就放哪一条"（与录音 / 转写同一理由）：带路径就先选中再切格。
          if (intent.path) {
            setSelectedPath(intent.path);
          }
          setLocation(intent.location);
          break;
        case "chat":
          setLocation(intent.location);
          if (intent.path) {
            void chatController.openHistoryEntry(intent.path);
          }
          break;
        case "apps":
          setAppsSelection(intent.appId);
          setLocation(intent.location);
          break;
        case "teleprompter":
          // 提词器应用未生效：深链落到它的详情页（ADR-165 ④ 同一条）。
          if (appOfUnavailableSpaceRef.current.has("teleprompter")) {
            setAppsSelection("teleprompter" satisfies AppId);
            setLocation(locationFor("apps"));
            break;
          }
          setLocation(intent.location);
          break;
        case "creator":
          // 创作应用未启用：深链落到它的详情页（ADR-165 ④），不打开任何创作的东西。
          if (appOfUnavailableSpaceRef.current.has("creator")) {
            setAppsSelection("creator" satisfies AppId);
            setLocation(locationFor("apps"));
            break;
          }
          // 创作空间（ADR-165 ⑥）：看板、灵感库，或某个项目的某个页签——都在创作空间里。
          setCreatorView(creatorViewOf(intent.target));
          setLocation(intent.location);
          break;
        case "note": {
          // 行锚（`#L12`）由问答出处跳转带来（PRD-16-012）：先打开笔记，再经既有
          // `luna:editor-reveal` 定位——那条管线本就支持"目标尚未加载时暂存待兑现"。
          const target = intent.target;
          if (target?.path) {
            // 产物护栏（ADR-096 ③）：存量 `notes://` 深链（旧通知、旧引用）指向产物时
            // 让路到知识库，不进编辑桌。行锚随之无处兑现——产物的行级定位归 D2 阅读视图。
            if (productPathsRef.current.has(target.path)) {
              setTopicSelection(target.path);
              setLocation(locationFor("knowledge", "topics"));
              break;
            }
            setPanesState((state) => openNote(state, target.path));
            setAutoFocusTitle(true);
            if (target.line !== null) {
              // 暂存 + 广播：深链目标多半还没打开（否则不需要深链），纯事件必然丢失。
              requestEditorReveal({ path: target.path, line: target.line, headingIndex: null, focusEditor: true });
            }
          }
          setLocation(intent.location);
          void refreshVault();
          break;
        }
        case "topic":
          // 知识产物深链（ADR-096 ③）：落主题页分区，带路径时顺带选中那一页。
          if (intent.path) {
            setTopicSelection(intent.path);
          }
          setLocation(intent.location);
          break;
      }
    };
    const onRecordingArchived = () => {
      void refreshVault();
    };
    window.addEventListener(NAVIGATE_EVENT, onNavigate);
    window.addEventListener("luna:recording-archived", onRecordingArchived);
    // 录音指示：先拉一次真值（主窗热重载时录音可能正在跑），之后只吃控制器的转移广播。
    void commands.recordingState().then((state) => {
      setRecordingIndicator(recordingIndicatorFrom(state, Date.now()));
    });
    const unlistenRecording = events.recordingStateChanged.listen((event) => {
      setRecordingIndicator(recordingIndicatorFrom(event.payload, Date.now()));
    });
    return () => {
      window.removeEventListener(NAVIGATE_EVENT, onNavigate);
      window.removeEventListener("luna:recording-archived", onRecordingArchived);
      void unlistenRecording.then((unlisten) => unlisten());
    };
  }, []);

  // 命令注册表是快捷键与展示的单一来源（TDD-07 §4.2）：按键判定用各自的谓词，
  // 执行一律经 `commandById(...).run(context)`，上下文由 shell 一处提供。
  const commandContext = useMemo<CommandContext>(
    () => ({
      openQuickSwitcher: () => setQuickSwitcherOpen(true),
      openCommandPalette: () => setCommandPaletteOpen(true),
      // 侧栏开合只对有列表的目的地有意义（否则无可收）；判据留在 shell，命令面板照常列它。
      toggleSidebar: () => {
        if (hasSidebar(locationRef.current.space)) {
          setSidebarCollapsed((collapsed) => !collapsed);
        }
      },
      // 经 ref 取**此刻**的实现：上下文是 memo 化的，直接捕获会钉住首帧的 `t` 与打开逻辑。
      createNewNote: () => {
        void createNewNoteRef.current();
      },
      // 同上：文字速记要读**此刻**的库状态才能如实回报终态，直接捕获会钉住首帧的
      // `vaultOpen`（那时它还是 `null`）。
      captureTextQuickNote: () => {
        void captureTextRef.current();
      },
      splitRight: () => {
        setPanesState((state) =>
          state.panes.length < MAX_PANES ? splitPane(state, state.activePaneId) : state,
        );
      },
      openEmptyTab: () => {
        setPanesState((state) => openEmptyTab(state, state.activePaneId));
      },
      // ⌘⇧T（PRD-07-035 W2b）：站在别的空间时先回到编辑区，否则重开了也看不见。
      reopenClosedTab: () => {
        if (panesStateRef.current.closed.length === 0) {
          return;
        }
        if (!hasEditorSurface(locationRef.current)) {
          go("notes");
        }
        setPanesState((state) => reopenClosedTab(state));
      },
      openOverview: () => setDashboardOpen(true),
      // 创作命令组（ADR-165 ⑤）：与侧栏、看板同一条实现，经 ref 取此刻的那一份。
      createCreatorProject: () => {
        void creatorActionsRef.current.createProject();
      },
      createSwipe: () => {
        void creatorActionsRef.current.createSwipe();
      },
      openSwipeLibrary: () => {
        setCreatorView({ view: "swipes" });
        go("creator");
      },
      openCreatorProjectTeleprompter: () => {
        void creatorActionsRef.current.openTeleprompter();
      },
      openActiveNoteInTeleprompter: () => {
        const path = activeTabPathRef.current;
        if (!path) return;
        void openInTeleprompter(path).then((failure) => {
          if (failure) setStatus(failure);
        });
      },
      createTeleprompterDraft: (fromClipboard) => {
        void createTeleprompterDraft({ kind: fromClipboard ? "clipboard" : "empty" }).then((failure) => {
          if (failure) setStatus(failure);
        });
      },
      openCreatorProjectPublish: () => {
        const view = creatorViewRef.current;
        if (view.view !== "project") return;
        setCreatorView({ view: "project", home: view.home, pane: { tab: "publish" } });
        go("creator");
      },
      goToSpace: (space) => {
        go(space);
      },
      openVaultSearch: () => {
        go("search");
        setSidebarCollapsed(false);
        window.requestAnimationFrame(() => {
          // 限定在搜索面板内取那一个输入框：查询框已是全应用共用原语，不限定作用域会
          // 命中屏幕上任何一个（图谱侧栏、设置侧栏都在用同一个类）。
          document.querySelector<HTMLInputElement>(".luna-search .luna-query-box__input")?.focus();
        });
      },
    }),
    [],
  );

  // 工作区快捷键只挂一个监听：命中哪条有键的命令由注册表判（`workspaceCommandForKey`），
  // 执行一律 `run(commandContext)`；可配置键位与双击时间窗都来自设置真值。
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const command = workspaceCommandForKey(event, shortcutHints, {
        detector: doubleTapDetector.current,
        windowMs: doubleTapWindowMs,
        nowMs: event.timeStamp,
      });
      if (!command) {
        return;
      }
      event.preventDefault();
      command.run(commandContext);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [commandContext, shortcutHints, doubleTapWindowMs]);

  // 创作命令组要的几件事（命令上下文是 memo 化的，经 ref 取此刻的实现）。
  const creatorViewRef = useRef(creatorView);
  creatorViewRef.current = creatorView;
  const creatorActionsRef = useRef({
    createProject: async () => {},
    createSwipe: async () => {},
    openTeleprompter: async () => {},
  });
  creatorActionsRef.current = {
    createProject: () => createCreatorProject(),
    createSwipe: () => createSwipe(),
    // 当前项目的脚本由 Rust 判（骨架那篇或最近改过的一篇）；没有脚本就去概览写。
    openTeleprompter: async () => {
      const view = creatorViewRef.current;
      if (view.view !== "project") return;
      const reply = await commands.creatorProject(view.home);
      const script = reply.status === "ok" ? reply.data.script : null;
      if (script === null) {
        setCreatorView({ view: "project", home: view.home, pane: { tab: "overview" } });
        go("creator");
        return;
      }
      await commands.teleprompterOpen(script, false);
    },
  };

  const createNewNoteRef = useRef<() => Promise<WorkspaceMetricEntryResultDto>>(() =>
    Promise.resolve("failed" as const),
  );
  async function createNewNote(): Promise<WorkspaceMetricEntryResultDto> {
    const result = await commands.createNote("note");
    if (result.status === "ok") {
      openNoteInPanes(result.data.path);
      setAutoFocusTitle(true);
      await refreshVault();
      setStatus(t("status.ready"));
      return "arrived";
    }
    setStatus(t("status.error"));
    return "failed";
  }
  createNewNoteRef.current = createNewNote;

  /**
   * 新建视频项目（ADR-162 ② / ADR-165 ⑥）：新建项目对话框一次问项目名、发到哪些渠道与计划发布
   * 日期（S6c），文件由 Rust 建好，建好留在创作空间的项目首页——不再被带去「笔记」。
   */
  async function createCreatorProject() {
    setNewProjectDialog({ busy: false, error: null });
  }

  async function submitNewProject(name: string, options: NewProjectOptionsDto) {
    setNewProjectDialog({ busy: true, error: null });
    const result = await commands.creatorCreateProject(name, options);
    if (result.status !== "ok") {
      setNewProjectDialog({ busy: false, error: vaultErrorText(result.error) });
      return;
    }
    setNewProjectDialog(null);
    setCreatorView({ view: "project", home: result.data.path, pane: { tab: "overview" } });
    go("creator");
    await refreshVault();
    setStatus(t("status.ready"));
  }

  /** 新建一条灵感（`type: swipe`，落创作目录下的灵感文件夹），在创作空间里就地编辑。 */
  async function createSwipe() {
    const result = await commands.createNote("swipe");
    if (result.status === "ok") {
      setCreatorView({ view: "note", path: result.data.path });
      go("creator");
      setAutoFocusTitle(true);
      await refreshVault();
      setStatus(t("status.ready"));
      return;
    }
    setStatus(t("status.error"));
  }

  /**
   * 创作空间里的编辑器（ADR-165 ⑥）：与「笔记」空间同一个组件，只是链接在创作空间里打开
   * （`openPath` 由项目工作区给：主页 / 研究 / 脚本回各自页签，其余作为临时页签），产物照样
   * 让路到知识库（ADR-096 ③）。改名后主区跟着新路径走。
   */
  const renderCreatorEditor = (path: string, openPath: (path: string) => void) => (
    <VaultEditor
      key={path}
      path={path}
      canvasDestination="notes"
      autoFocusTitle={autoFocusTitle}
      onTitleFocused={() => setAutoFocusTitle(false)}
      onOpenPath={(opened) => {
        if (routeIfKnowledgeProduct(opened)) return;
        openPath(opened);
      }}
      onOpenTag={openTagView}
      onSavedPath={(saved, previous) => {
        if (previous) setCreatorView((view) => remapCreatorView(view, previous, saved));
        void refreshVault();
      }}
      onStatus={setStatus}
      onEditorMetrics={setEditorMetrics}
    />
  );

  /**
   * 文字速记入口的**唯一实现**（PRD-06-058 B / PRD-07-067 ②）：跳到「速记」、展开侧栏、
   * 展开那里的「快速捕获」并把光标放进去。「添加内容」浮层与命令面板都走它，不各写一份。
   *
   * **聚焦必须显式做**：用户已经站在「速记」时再点这一项，`go()` 是空操作、组件不重挂载，
   * 挂载副作用不会再触发——屏幕上什么都不发生，而指标那一侧照记一次「到达」。2026-10-08
   * 起文字捕获默认收起（R1），故这里发一次展开请求（`captureOpenRequest` +1），由捕获胶囊
   * 展开并聚焦，而不是去查一个此刻可能不存在的输入框。
   *
   * **终态如实回报**：库没打开时那个框存在也存不下东西，那是一次 `rejected`。恒记
   * `arrived` 会让入口到达率（PRD-07-070）**只可能是 100%**。
   */
  const captureTextRef = useRef<() => Promise<WorkspaceMetricEntryResultDto>>(() =>
    Promise.resolve("failed" as const),
  );
  function captureTextQuickNote(): Promise<WorkspaceMetricEntryResultDto> {
    go("quick_notes");
    setSidebarCollapsed(false);
    setCaptureOpenRequest((request) => request + 1);
    return Promise.resolve(vaultOpen === false ? "rejected" : "arrived");
  }
  captureTextRef.current = captureTextQuickNote;

  /** 唤起录音 HUD（不自动开始采音；与全局热键共用同一常驻面板，PRD-04-001）。 */
  async function showRecordingHud(): Promise<WorkspaceMetricEntryResultDto> {
    const result = await commands.recordingShowHud();
    if (result.status === "error") {
      setStatus(t("status.error"));
      return "failed";
    }
    return "arrived";
  }

  /** 导入音/视频入统一处理队列（PRD-05-030）；有入队即展开队列面板反馈进度。 */
  async function startImport(): Promise<WorkspaceMetricEntryResultDto> {
    // 选文件对话框被关掉 → 一件都没入队。那是**取消**，不是失败——把用户的主动放弃
    // 记成失败，会让「入口到达率」看起来像产品坏了。
    if ((await materialImport.start()) > 0) {
      setQueueOpen(true);
      return "arrived";
    }
    return "cancelled";
  }

  /** 「从链接导入」入队成功（PRD-05-006）：开队列面板 + 状态栏如实报件数。 */
  function onUrlQueued(count: number) {
    setQueueOpen(true);
    setStatus(tTranscribe("fetch.queued", { count }));
  }

  /** 跳到设置页某处（抓取工具路径 / 隐私开关等就地可达，不让用户自己去找）；解析归 SettingsView。 */
  function openSettingsRoute(route: string) {
    setSettingsFocus((current) => ({ route, nonce: (current?.nonce ?? 0) + 1 }));
    go("settings");
  }

  /**
   * 五类创建动作的执行上下文（PRD-07-069）。
   *
   * **这里一条业务都不写**：全部复用既有 handler。「添加内容」与「快速开始」是两个
   * 呈现面，不是两套执行栈——刻意没有引入 `execute_create_action` 那样的通用执行命令
   * （TDD-07 §2.2），因为每个动作本来就有自己的权威命令，再包一层只会多一处要同步的判定。
   */
  // 工作区到访观测（PRD-07-070）：**只在这一个导航宿主发送**——两个展示面各报一次，
  // 同一次到访会被记成两次（TDD-12 §2.1）。授权判定在 Rust，关着时命令静默 no-op。
  useSurfaceMetrics(location);

  const createActionContext: CreateActionContext = {
    // 文字速记：落到「速记」并把光标放进那里的捕获框（`captureTextQuickNote`，与命令
    // 面板共用那一份实现）——不必在 Popover 里嵌第二个编辑器（PRD-07-067 ②）。
    captureText: () => captureTextQuickNote(),
    // 入口标注的去向是「速记」，故 HUD 这一次的默认落点也必须是速记（PRD-02-020）。
    captureVoice: () => showVoiceInputHud("quick_note"),
    createNote: () => createNewNote(),
    startRecording: () => showRecordingHud(),
    importMaterial: () => startImport(),
  };

  /**
   * 唤起语音输入 HUD。
   *
   * **`preset` 必填、没有缺省值**（TDD-07 §2.3 明写"无缺省参数兼容"）：普通入口显式
   * 传 `current`（用用户的持久默认落点），「语音速记」那一族入口传 `quick_note`。
   * 留一个 `= "current"` 的代价是"忘了传"与"就是要 current"在调用点长得一模一样——而
   * 这两者的区别正是 PRD-07-069 的全部内容：入口标注的产物去向必须与 HUD 的默认动作
   * 一致，否则用户从「语音速记」进来说完话，结果被注入到了别的应用里。
   */
  async function showVoiceInputHud(
    preset: VoiceInputHudPresetDto,
  ): Promise<WorkspaceMetricEntryResultDto> {
    const result = await commands.voiceInputShowHud(preset);
    if (result.status === "ok") {
      return "arrived";
    }
    setStatus(t("status.error"));
    // 已有会话在跑不是失败：HUD 照样显示出来了，用户回到的正是那个会话（TDD-07 §2.3）。
    // 把它记成 failed 会让这条指标把一次正常的"回到原会话"算成产品出错。
    return result.error.startsWith("VOICE_SESSION_BUSY") ? "rejected" : "failed";
  }

  // ── 记录收件箱的三条动线（PRD-06-058 B/C/D）─────────────────────────────
  // 全部只发命令、消费返回的事实：落盘形态、成熟度判定与拒绝理由都在 Rust（ADR-003）。

  /**
   * 文字快速捕获：回车即落袋；`open` = ⌘/Ctrl+回车，落袋后打开编辑器。
   *
   * 与空白编辑器画布**同一条命令、同一个目的地事实**（PRD-06-058 P0-1）：捕获框长在
   * 「记录」里，故它恒传 `"drafts"`——这不是分支，是这块界面的常量。
   */
  async function captureQuickNote(text: string, options: { open: boolean }): Promise<boolean> {
    const result = await commands.createNoteFromText(text, "drafts");
    if (result.status === "error") {
      setStatus(vaultErrorText(result.error));
      return false;
    }
    await refreshVault();
    if (options.open) {
      openNoteInPanes(result.data.path);
    } else {
      // 不打开时仍选中它：捕获框下面那一行立刻出现且是选中的，用户看得见东西落地了。
      setSelectedPath(result.data.path);
    }
    return true;
  }

  /**
   * 转为正式笔记（速记会搬进 `notes_dir`）。
   *
   * 侧栏行上那枚 ✓ 与主区「整理」栏那颗按钮**都走这里**：转正之后主区清空、撤销条挂上，
   * 两个入口于是给出同一套收尾（此前主区那条路一样也不做，转正之后无从撤销）。
   */
  async function settleDraft(path: string): Promise<void> {
    const result = await commands.settleNoteAsKnowledge(path);
    if (result.status === "error") {
      setStatus(vaultErrorText(result.error));
      return;
    }
    await refreshVault();
    setStatus(t("draft.settled"));
    // 撤销的对象是**转正后**的路径（文件搬过家了），不是原路径。
    setUndoSettlePath(result.data);
    setSelectedPath(null);
  }

  /**
   * 撤销「转为正式笔记」——`promote` 的逆操作：搬回速记流、换回 `type: quick-note`
   * （PRD-06-058 验收「可撤销反馈」）。
   *
   * **不是「标记为待整理」**：那条路只加 `luna.stage` 一行，产物是一篇待整理的普通笔记，
   * 留在「笔记」空间；而用户撤销的是"把速记转正"这个动作，他要的是那条速记回到速记流。
   */
  async function demoteQuickNote(path: string): Promise<boolean> {
    const result = await afterEditorsSaved(
      () => commands.demoteToQuickNote(path),
      () => ({ status: "error" as const, error: tCommon("saveBlocksMove") }),
    );
    if (result.status === "error") {
      setStatus(vaultErrorText(result.error));
      return false;
    }
    await refreshVault();
    setStatus(t("draft.undone"));
    return true;
  }

  /**
   * 点一级图标：再点**当前空间** = 收/开主侧栏（VS Code 心智，PRD-07-057 ③）；
   * 否则切到那个空间的默认分区。
   */
  function selectSpace(target: WorkspaceSpace) {
    if (target === space && hasSidebar(target)) {
      setSidebarCollapsed((collapsed) => !collapsed);
      return;
    }
    go(target);
  }

  async function showPopupTranslate() {
    const result = await commands.spikeShowPopup();
    if (result.status === "error") {
      setStatus(t("status.error"));
    }
  }

  async function showCaptionHud() {
    const result = await commands.captionShowHud();
    if (result.status === "error") {
      setStatus(t("status.error"));
    }
  }


  /**
   * 归档类笔记的删除通道（速记 / 录音 / 转写 / 标签结果共用）：确认 → `vault_delete`
   * → 清选中 → 刷新。此前三个视图各写一份同形状的函数，右键菜单再来一份就是第四份。
   * 多选即整批：一次确认、一条命令（剪枝、幂等与逐项成败都归 Rust），收尾只作用于
   * **真正删掉的那些**。
   */
  async function deleteArchivedNotes(
    paths: readonly string[],
    confirm: ConfirmOptions,
    afterDeleted: (deleted: readonly string[]) => void,
  ) {
    const deleted = await deleteNotesWithConfirm({
      paths,
      confirm,
      ask,
      onDeleted: afterDeleted,
      onError: () => setStatus(t("status.error")),
    });
    if (!deleted) {
      return;
    }
    await refreshVault();
    setStatus(t("status.ready"));
  }

  /**
   * 归档类列表行的右键菜单（PRD-07-004 / PRD-07-059）：条目集来自共享构建器，
   * 各视图只声明"这一类对象有没有转写能力、删除时怎么问、删完清哪个选中"。
   */
  // 文件夹能力（PRD-07-062）：三条清单各一份实例（归档目录与条目集各不相同），
  // 但实现只有 `use-note-folders.ts` 一处，动作全部落在既有 Vault 命令上。
  const recordingFolders = useNoteFolders({
    noteType: "recording",
    items: recordingNotes,
    selectedPath: selectedRecordingPath,
    onStatus: setStatus,
    onChanged: () => void refreshVault(),
    deleteConfirm: (path) => ({
      title: t("folder.deleteConfirm", { path }),
      description: deleteConsequence,
      confirmLabel: tCommon("action.delete"),
    }),
  });
  // 速记同样有文件夹（PRD-07-062）：它的清单基座与录音/转写不同（时间桶），但"文件夹是
  // Vault 里的目录"这件事没有差别，故用同一个 hook、同一批 Vault 命令。
  /**
   * 速记的多选（PRD-06-059）：与笔记树、录音、转写、标签结果同一套原语。
   *
   * 此前这一格自带一套行首勾选框——同一件事在应用里于是有两种做法与两种外观
   * （2026-09-09 用户裁决：统一为 Cmd/Ctrl 加选、Shift 连选，批量动作走右键菜单）。
   */
  const [draftRows, setDraftRows] = useState<readonly string[]>([]);
  const draftSelection = useNoteSelection({
    current: selectedDraftPath,
    visible: draftRows,
    onDelete: () => deleteDraftSelection(),
  });

  const draftFolders = useNoteFolders({
    noteType: "quick-note",
    items: draftList.items,
    selectedPath: selectedDraftPath,
    onStatus: setStatus,
    onChanged: () => void refreshVault(),
    deleteConfirm: (path) => ({
      title: t("folder.deleteConfirm", { path }),
      description: deleteConsequence,
      confirmLabel: tCommon("action.delete"),
    }),
  });
  /**
   * AI 对话历史的多选（PRD-06-059）：与另外五条清单同一套原语。
   *
   * 会话归档也是库里的一篇 Markdown——"选几篇一起删 / 一起蒸馏"在这里同样成立，而此前
   * 这条清单连加选都没有（真机 2026-09-09）。
   */
  const [chatRows, setChatRows] = useState<readonly string[]>([]);
  const chatSelection = useNoteSelection({
    current: chatNotePath,
    visible: chatRows,
    onDelete: () => deleteChatSelection(),
  });

  const chatFolders = useNoteFolders({
    noteType: "chat",
    items: chatController.history.entries,
    selectedPath: chatNotePath,
    onStatus: setStatus,
    onChanged: () => void chatController.history.refresh(),
    deleteConfirm: (path) => ({
      title: t("folder.deleteConfirm", { path }),
      description: deleteConsequence,
      confirmLabel: tCommon("action.delete"),
    }),
  });
  const transcribeFolders = useNoteFolders({
    noteType: "transcript",
    items: transcribeNotes,
    selectedPath: selectedTranscribePath,
    onStatus: setStatus,
    onChanged: () => void refreshVault(),
    deleteConfirm: (path) => ({
      title: t("folder.deleteConfirm", { path }),
      description: deleteConsequence,
      confirmLabel: tCommon("action.delete"),
    }),
  });

  /** 删除录音清单里选中的这一批（工具栏按钮与 Delete 键共用）。 */
  function deleteRecordingSelection() {
    const paths = recordingSelection.ordered;
    if (paths.length === 0) {
      return;
    }
    void deleteArchivedNotes(
      paths,
      // 多选走数得清的那句共享文案；单选仍用录音自己的措辞（它还要交代"音频文件一并
      // 删除"这件只有录音才有的事）。
      paths.length > 1
        ? {
            title: tCommon("menu.deleteSelectedTitle", { count: paths.length }),
            description: t("main.deleteRecordingDesc"),
            confirmLabel: tCommon("action.delete"),
          }
        : {
            title: t("main.deleteRecordingTitle", { path: paths[0] }),
            description: t("main.deleteRecordingDesc"),
            confirmLabel: tCommon("action.delete"),
          },
      () => setSelectedRecordingPath(null),
    );
  }

  /** 删除「速记」清单里选中的这一批（Delete 键与右键菜单同一条通道、同一张确认卡）。 */
  function deleteDraftSelection() {
    const paths = draftSelection.ordered;
    if (paths.length === 0) {
      return;
    }
    void deleteArchivedNotes(
      paths,
      paths.length > 1
        ? {
            title: tCommon("menu.deleteSelectedTitle", { count: paths.length }),
            description: deleteConsequence,
            confirmLabel: tCommon("action.delete"),
          }
        : {
            title: t("main.deleteQuickNoteTitle", { path: paths[0] }),
            description: deleteConsequence,
            confirmLabel: tCommon("action.delete"),
          },
      () => setSelectedPath(null),
    );
  }

  /**
   * 删除「AI 对话」清单里选中的这一批（Delete 键、右键菜单与行内按钮同一条通道）。
   *
   * 与另外四条清单同一条共享删除通道（一次确认、一条命令），只是会话多一步收尾：删掉的
   * 那批里含正开着的那篇时要换成新对话——那一步归 `use-chat-controller`。
   */
  function deleteChatSelection() {
    const paths = chatSelection.ordered;
    if (paths.length === 0) {
      return;
    }
    void deleteChatEntries(paths);
  }

  /** 删除这些会话（上面那条与行内按钮共用）。确认、收尾与"删掉的正是开着的那篇"归控制器。 */
  async function deleteChatEntries(paths: readonly string[]) {
    const removal = await chatController.deleteHistoryEntries(paths);
    setStatus(removal.failed ? t("status.error") : t("status.ready"));
  }

  /** 删除「转写」清单里选中的这一批（同上）。 */
  function deleteTranscribeSelection() {
    const paths = transcribeSelection.ordered;
    if (paths.length === 0) {
      return;
    }
    void deleteArchivedNotes(
      paths,
      paths.length > 1
        ? {
            title: tCommon("menu.deleteSelectedTitle", { count: paths.length }),
            description: tTranscribe("deleteDesc"),
            confirmLabel: tCommon("action.delete"),
          }
        : {
            title: tTranscribe("deleteTitle", { path: paths[0] }),
            description: tTranscribe("deleteDesc"),
            confirmLabel: tCommon("action.delete"),
          },
      () => setSelectedTranscribePath(null),
    );
  }

  /**
   * 项目菜单（ADR-167 ①②）动作之后的导航与刷新：侧栏、看板与项目页头共用这一份。正看着的项目被
   * 改名就跟到新主页，被删（或正看着的笔记在被删的文件夹里）就回看板。
   */
  const creatorProjectNav: ProjectMenuNav = {
    onOpen: (home, tab) => setCreatorView({ view: "project", home, pane: { tab } }),
    onRenamed: (from, to) => {
      setCreatorView((view) =>
        view.view === "project" && view.home === from
          ? { view: "project", home: to, pane: view.pane.tab === "doc" ? { tab: "overview" } : view.pane }
          : view,
      );
      creatorBoard.reload();
    },
    onDeleted: (home, targets) => {
      const gone = (path: string) => targets.some((target) => path === target || path.startsWith(`${target}/`));
      setCreatorView((view) => {
        if (view.view === "project" && (view.home === home || gone(view.home))) return { view: "board" };
        if (view.view === "note" && gone(view.path)) return { view: "board" };
        return view;
      });
      creatorBoard.reload();
    },
    onChanged: () => creatorBoard.reload(),
  };

  /** 创作里一篇笔记（灵感、项目里的东西）的菜单：与笔记空间同一份笔记菜单（ADR-167 ①）。 */
  function openCreatorNoteMenu(event: ReactMouseEvent, path: string, leading?: ContextMenuEntry[]) {
    openArchivedNoteMenu(event, path, {
      leading,
      confirm: {
        title: t("main.deleteNoteTitle"),
        description: deleteConsequence,
        confirmLabel: tCommon("action.delete"),
      },
      onDeleted: () =>
        setCreatorView((view) => {
          if (view.view === "note" && view.path === path) return { view: "swipes" };
          if (view.view === "project" && view.pane.tab === "doc" && view.pane.path === path) {
            return { view: "project", home: view.home, pane: { tab: "overview" } };
          }
          return view;
        }),
      onRenamed: (to) =>
        setCreatorView((view) => {
          if (view.view === "note" && view.path === path) return { view: "note", path: to };
          if (view.view === "project" && view.pane.tab === "doc" && view.pane.path === path) {
            return { view: "project", home: view.home, pane: { tab: "doc", path: to } };
          }
          return view;
        }),
    });
  }

  function openArchivedNoteMenu(
    event: ReactMouseEvent,
    path: string,
    options: {
      /** 本次动作的对象集（多选清单交它的选择集；缺省 = 只有 `path`）。 */
      paths?: readonly string[];
      /** 整批可「汇总成一篇」（记录收件箱的勾选集才成立，资格由 Rust 下发）。 */
      mergeable?: boolean;
      transcript?: boolean;
      /** 资料区笔记（录音 / 转写 / 对话）：给「生成知识笔记」（PRD-06-044）。 */
      distillable?: boolean;
      /** 语音速记：给「升级为笔记」（PRD-02-055 收件箱闭环）。 */
      promotable?: boolean;
      /** 速记此刻钉在桌面上没有（ADR-119）：据此给「钉到桌面」或「取消钉住」。 */
      pinnedToDesktop?: boolean;
      /** 记录态笔记：给「沉淀为知识」（PRD-06-053）。 */
      settleable?: boolean;
      /** 沉淀后的新路径（速记沉淀会搬进 `notes_dir`）。 */
      onSettled?: (to: string) => void;
      /** 重命名后的新路径（跟随选中，避免选中项指向一个已不存在的路径）。 */
      onRenamed?: (to: string) => void;
      /** 这一行专有、排在最前的几项（如创作来源的「移出项目」）。 */
      leading?: ContextMenuEntry[];
      confirm: ConfirmOptions;
      onDeleted: () => void;
      folders?: NoteFolders;
    },
  ) {
    // 产物行（标签清单、搜索命中里的主题页/知识地图）不承诺编辑桌（ADR-096 ③）：
    // 「在笔记库中打开」换成「在知识库中打开」，独立窗口项不给——闸口反正会转发，
    // 但菜单不该先说一句注定不兑现的话。
    const product = productPaths.has(path);
    const targets = options.paths?.length ? options.paths : [path];
    contextMenu.open(
      event,
      noteMenu({
        path,
        paths: targets,
        leading: options.leading,
        mergeable: options.mergeable,
        openInNotes: product ? undefined : () => openNoteInPanes(path),
        openInKnowledge: product ? () => openKnowledgeProduct(path) : undefined,
        openInWindow: product ? undefined : () => void openNoteWindow(path),
        moveTo: options.folders?.moveTo,
        // 重命名在这里**不给行内输入态**：这些清单的行是"标题 + 元信息 + 徽标"的复合行，
        // 把它整行换成一个输入框会让人当场失去上下文——它们走那张共享输入卡。
        rename: { onRenamed: options.onRenamed },
        transcript: options.transcript
          ? {
              // 点下去那一刻读一次队列，不为一次右键订阅进度。
              busy: currentProcessingQueue().some(
                (task) => task.note_rel === path && isTaskActive(task),
              ),
            }
          : undefined,
        distillable: options.distillable,
        promotable: options.promotable,
        pinnedToDesktop: options.pinnedToDesktop,
        settleable: options.settleable,
        onSettled: options.onSettled,
        remove: { confirm: options.confirm, onDeleted: options.onDeleted },
        onStatus: setStatus,
        onChanged: () => void refreshVault(),
      }),
    );
  }

  /**
   * 主题页目录行的菜单（PRD-07-059a）：**笔记条目集 + 本页专有动作的能力开关**。
   *
   * 走的是与文件树、四条采集清单、标签结果、搜索命中完全相同的那一个构建器
   *（`useNoteMenu`）——不为这一份新列表另写一套条目，否则同一条笔记在主题页分区与
   * 文件树里能做的事会悄悄分叉。专有动作全部落**既有**命令：针对本页体检 = 打开既有报告
   * 并按本页过滤（不新增单页体检命令）、转为我的笔记 / 舍弃改动 = 复核态两出口。
   */
  function openTopicRowMenu(event: ReactMouseEvent, row: TopicDirectoryRowDto) {
    const path = row.path;
    contextMenu.open(
      event,
      noteMenu({
        path,
        // 目录行本身就是产物：只给知识库打开项（ADR-096 ③）。
        openInKnowledge: () => openKnowledgeProduct(path),
        topicPage: {
          checkup: () => requestKnowledgeCheckup(path),
          // 「转为我的笔记」两态可用（复核态下它就是「保留我的改动」，同一条命令）；
          // 「舍弃改动」只在「已被修改」态给——判定是 Rust 下发的 `ownership`。
          modified: row.ownership === "user_owned",
          forkNote: () => {
            void commands.topicForkNote(path).then((result) => {
              setStatus(
                result.status === "ok"
                  ? t("topic.reviewKeepDone", { title: noteStem(result.data) })
                  : result.error,
              );
            });
          },
          reviewDiscard:
            row.ownership === "user_owned"
              ? () => {
                  void (async () => {
                    // 两步确认（ADR-096 ④）：第二步把后果讲全——版本历史仍能找回。
                    if (
                      !(await ask({
                        title: t("topic.reviewDiscardYes"),
                        description: t("topic.reviewDiscardConfirm"),
                        confirmLabel: t("topic.reviewDiscardYes"),
                      }))
                    ) {
                      return;
                    }
                    const result = await commands.topicReviewDiscard(path);
                    setStatus(
                      result.status === "ok" ? t("topic.reviewDiscardDone") : result.error,
                    );
                  })();
                }
              : undefined,
        },
        remove: {
          confirm: {
            title: t("main.deleteNoteTitle"),
            description: deleteConsequence,
            confirmLabel: tCommon("action.delete"),
          },
          // 删掉的正是阅读中的那一页：回首页，而不是对着一页已不存在的内容。
          onDeleted: (deleted) =>
            setTopicSelection((current) =>
              current !== null && deleted.includes(current) ? null : current,
            ),
        },
        onStatus: setStatus,
        onChanged: () => void refreshVault(),
      }),
    );
  }

  // 列表组件从主区上移至此，回调复用既有 handler（ADR-003：前端只编排布局，不搬业务）。
  /**
   * 主侧栏列表：按**位置**分发。
   *
   * 有分区的空间用 `<space>/<section>` 作键，无分区的用 `<space>`——键由
   * `sidebarKeyOf(location)` 一处生成，故不存在"某个分区忘了给列表"这种只在切过去时
   * 才看得见的漏法（缺键当场编译不过）。
   */
  function renderSidebar(): ReactNode {
    if (!hasSidebar(space)) {
      return null;
    }
    const renderers: Record<SidebarKey, () => ReactNode> = {
      notes: () => (
        <NoteTree
          tree={tree}
          selectedPath={selectedPath}
          onSelect={(path, options) => {
            if (path === null) {
              setSelectedPath(null);
              return;
            }
            // 文件树单击 = 浏览（预览标签）；双击 / ⌘ / 中键由树传来明确意图。
            setPanesState((state) =>
              openNote(
                state,
                path,
                options?.newTab ? { newTab: true } : { as: options?.keep ? intentAs : browseAs },
              ),
            );
          }}
          onCreateNote={() => void createNewNote()}
          onOpenTemplatePicker={() => setTemplatePickerOpen(true)}
          onStatus={setStatus}
          onMoved={(from, to) => {
            setPanesState((state) => remapPath(state, from, to));
          }}
          onDeleted={(path) => {
            setPanesState((state) => closeTabsUnder(state, path));
          }}
          onOpenInSplit={openNoteInSplit}
          onOpenInWindow={(path) => void openNoteWindow(path)}
        />
      ),
      search: () => (
        <SearchView
          controller={searchController}
          searchShortcut={shortcutHints["shortcuts.vault_search"] ?? ""}
          // 搜索与笔记共用编辑桌（PRD-07-058）故不经 openNoteInPanes（那会切走空间）；
          // 产物闸口必须单独过（真机第六轮：这条通路曾把主题页开成可编辑）。
          onOpenPath={(path) => {
            if (routeIfKnowledgeProduct(path)) {
              return;
            }
            setPanesState((state) => openNote(state, path, { as: browseAs }));
          }}
          onOpenInSplit={openNoteInSplit}
          onOpenInWindow={(path) => void openNoteWindow(path)}
          onStatus={setStatus}
        />
      ),
      quick_notes: () => (
        <DraftInbox
          notes={draftList.items}
          loadState={draftList}
          // 页头徽标读**筛出来有几条**；活动栏角标读全库总数（`draftBadge`），两个数
          // 各有各的问题要答（PRD-07-066）。
          total={draftMeta.matched}
          filter={draftFilter}
          onFilterChange={setDraftFilter}
          query={draftQuery}
          onQueryChange={setDraftQuery}
          folders={draftFolders}
          footer={
            <ListLoadMore
              loading={draftList.loading}
              exhausted={draftList.exhausted}
              failed={draftList.failed}
              onLoadMore={draftList.loadMore}
              onRetry={() => void draftList.reload()}
            />
          }
          selectedPath={selectedPath}
          onSelect={setSelectedPath}
          onOpenInNotes={(path) => openNoteInPanes(path)}
          onSettle={(path) => void settleDraft(path)}
          onDistill={(path) => requestDistillNotes([path])}
          onCapture={captureQuickNote}
          captureOpenRequest={captureOpenRequest}
          onVoiceCapture={() => {
            // 这个 🎤 长在「速记」这一格上，故本次会话显式预设为速记
            // （PRD-02-020）：同一个捕获框的文字侧恒落速记，语音侧却跟随全局默认落点
            // （很可能是「注入光标」）就是入口自相矛盾——用户在同一个框里做同一件事
            // 得到两种结果。
            void showVoiceInputHud("quick_note");
          }}
          voiceShortcut={voiceInputShortcut ? formatShortcut(voiceInputShortcut) : ""}
          textShortcut={textQuickNoteShortcut ? formatShortcut(textQuickNoteShortcut) : ""}
          selection={draftSelection}
          onVisibleRows={setDraftRows}
          onItemContextMenu={(event, note, targets) =>
            openArchivedNoteMenu(event, note.path, {
              paths: targets,
              // 「合并为一篇」的三道判据全部来自 Rust：逐条资格 `can_merge` 与条数区间
              // `merge_min`/`merge_max`。前端不写第二份数字——抄一个 `>= 2` 在这里，就是
              // 同一条规则的第二处定义，而改了一边不会有任何东西变红。
              mergeable:
                targets.length >= draftMeta.merge_min &&
                targets.length <= draftMeta.merge_max &&
                targets.every((path) =>
                  draftList.items.some((item) => item.path === path && item.can_merge),
                ),
              // 「升级为笔记」只对速记成立（PRD-02-055）；被标记为记录的普通笔记
              // 已经在 `notes_dir` 里，它的毕业动作是「沉淀为知识」。判据来自 Rust。
              promotable: note.quick_note,
              // 钉到桌面 / 取消钉住（ADR-119）：同一个位置按此刻的状态给出两种说法。
              pinnedToDesktop: note.pinned_to_desktop,
              // 留了原始录音的速记可以重新转写（PRD-02-032）：判据由 Rust 下发。
              transcript: note.has_media,
              settleable: true,
              distillable: true,
              onSettled: (to) => setSelectedPath(to),
              onRenamed: (to) => setSelectedPath(to),
              confirm: {
                title: t("main.deleteQuickNoteTitle", { path: note.path }),
                description: deleteConsequence,
                confirmLabel: tCommon("action.delete"),
              },
              onDeleted: () => setSelectedPath(null),
            })
          }
        />
      ),
      recordings: () => (
        // 时长徽标与转写列表**同一个字段、同一份渲染**（PRD-07-007）：同基座的两个
        // 消费者此前能力不对等，而不对等的方向还错了——录音才是回访最频繁的一类。
        <FilteredNoteList
          noteType="recording"
          items={recordingNotes}
          loadState={recordingList}
          itemBadge={(item) => formatListDuration(item.duration_ms)}
          footer={
            <ListLoadMore
              loading={recordingList.loading}
              exhausted={recordingList.exhausted}
              failed={recordingList.failed}
              onLoadMore={recordingList.loadMore}
              onRetry={() => void recordingList.reload()}
            />
          }
          selectedPath={selectedRecordingPath}
          onSelect={setSelectedRecordingPath}
          selection={recordingSelection}
          onVisibleRows={setRecordingRows}
          rootDir={recordingFolders.rootDir}
          dirs={recordingFolders.dirs}
          onCreateFolder={recordingFolders.createFolder}
          onRenameFolder={recordingFolders.renameFolder}
          onDeleteFolder={recordingFolders.deleteFolder}
          onMoveInto={recordingFolders.moveInto}
          onItemContextMenu={(event, item, paths) =>
            openArchivedNoteMenu(event, item.path, {
              paths,
              transcript: true,
              distillable: true,
              // 「汇总成一篇」（PRD-06-047 修订：资料照样能当碎片）——这份清单的每一行
              // 都是该类型的一篇 `.md`，不可能是聚合页，故整批恒可汇总。
              mergeable: true,
              confirm: {
                title: t("main.deleteRecordingTitle", { path: item.path }),
                description: t("main.deleteRecordingDesc"),
                confirmLabel: tCommon("action.delete"),
              },
              onDeleted: () => setSelectedRecordingPath(null),
              onRenamed: (to) => setSelectedRecordingPath(to),
              folders: recordingFolders,
            })
          }
          itemMeta={(item) => <ArchiveRowMeta item={item} timeOf={rowTimeFor(recordingSort)} />}
          groupTime={timeGroupingFor(recordingSort)}
          filtering={recordingNeedle.length > 0}
          action={
            /* 与「转写」同一套侧栏模板（PRD-07-003 / PRD-07-007）：一枚主动作 + 搜索/排序一行。
               没有常驻的编辑与删除按钮：编辑 = 主区工具栏的「校对」，删除走行右键、多选
               批量菜单与 Delete 键——它们此前与主动作同尺寸并排，一颗还是红的。 */
            <div className="luna-capture-toolbar">
              <CaptureStartButton
                icon={<MicIcon size={14} />}
                label={t("capture.startRecording")}
                shortcut={recordingShortcut ? formatShortcut(recordingShortcut) : ""}
                onClick={() => void showRecordingHud()}
              />
              {(recordingNotes.length > 0 || recordingNeedle.length > 0) && (
                <ArchiveListFilters
                  query={recordingQuery}
                  onQueryChange={setRecordingQuery}
                  sort={recordingSort}
                  onSortChange={setRecordingSort}
                  searchLabel={t("archive.searchRecordings")}
                  sortLabels={{ created: t("archive.sortRecorded") }}
                />
              )}
            </div>
          }
          emptyState={
            recordingNeedle.length > 0 ? (
              <EmptyState
                icon={<SearchIcon size={24} />}
                title={t("capture.recordingNoMatchTitle")}
                description={tTranscribe("noMatchBody")}
                action={
                  <button
                    type="button"
                    className="luna-btn luna-btn--secondary"
                    onClick={() => setRecordingQuery("")}
                  >
                    {tTranscribe("clearFilter")}
                  </button>
                }
              />
            ) : (
              <EmptyState
                icon={<MicIcon size={24} />}
                title={t("capture.recordingEmptyTitle")}
                description={t("capture.recordingEmptyBody")}
                footnote={
                  recordingShortcut ? (
                    <>
                      {t("capture.shortcutHint")}
                      <kbd>{formatShortcut(recordingShortcut)}</kbd>
                    </>
                  ) : undefined
                }
              />
            )
          }
        />
      ),
      transcripts: () => (
        <TranscribeNoteList
          items={transcribeNotes}
          loadState={transcribeList}
          query={transcribeQuery}
          onQueryChange={setTranscribeQuery}
          sort={transcribeSort}
          onSortChange={setTranscribeSort}
          footer={
            <ListLoadMore
              loading={transcribeList.loading}
              exhausted={transcribeList.exhausted}
              failed={transcribeList.failed}
              onLoadMore={transcribeList.loadMore}
              onRetry={() => void transcribeList.reload()}
            />
          }
          selectedPath={selectedTranscribePath}
          onSelect={setSelectedTranscribePath}
          selection={transcribeSelection}
          onVisibleRows={setTranscribeRows}
          folders={transcribeFolders}
          onItemContextMenu={(event, item, paths) =>
            openArchivedNoteMenu(event, item.path, {
              paths,
              transcript: true,
              distillable: true,
              mergeable: true,
              confirm: {
                title: tTranscribe("deleteTitle", { path: item.path }),
                description: tTranscribe("deleteDesc"),
                confirmLabel: tCommon("action.delete"),
              },
              onDeleted: () => setSelectedTranscribePath(null),
              onRenamed: (to) => setSelectedTranscribePath(to),
              folders: transcribeFolders,
            })
          }
          itemMeta={(item) => <ArchiveRowMeta item={item} timeOf={rowTimeFor(transcribeSort)} />}
          importer={transcribeImport}
          onImport={() => void startImport()}
          onImportUrl={linkImportEnabled ? () => setUrlImportOpen(true) : undefined}
          onStatus={setStatus}
        />
      ),
      creator: () => (
        <CreatorSidebar
          board={creatorBoard}
          revision={vaultRevision}
          enabled={vaultReady}
          activePath={creatorActivePath(creatorView)}
          onOpen={(path) => setCreatorView({ view: "note", path })}
          onOpenProject={(path) => setCreatorView({ view: "project", home: path, pane: { tab: "overview" } })}
          onCreateProject={() => void createCreatorProject()}
          onCreateSwipe={() => void createSwipe()}
          onOpenLibrary={() => setCreatorView({ view: "swipes" })}
          projectNav={creatorProjectNav}
          onSwipeMenu={openCreatorNoteMenu}
        />
      ),
      chat: () => (
        <ChatHistoryList
          entries={chatController.history.entries}
          loadState={chatController.history.page}
          // 徽标是全库总数（Rust 的 `COUNT(*)`），不是已加载条数——与另外四条侧栏同一条规则。
          total={chatController.history.total}
          // 「新对话」也在别的应用里按得到（PRD-09 §2.6）——此前只有速记那颗提示了。
          shortcut={quickChatShortcut ? formatShortcut(quickChatShortcut) : ""}
          footer={
            <ListLoadMore
              loading={chatController.history.page.loading}
              exhausted={chatController.history.page.exhausted}
              failed={chatController.history.page.failed}
              onLoadMore={chatController.history.page.loadMore}
              onRetry={() => void chatController.history.refresh()}
            />
          }
          activePath={chatNotePath}
          canOpenInNotes
          onNewChat={() =>
            void chatController.startPresetChat(chatController.store.getSnapshot().presetId)
          }
          onOpen={(path) => void chatController.openHistoryEntry(path)}
          onDelete={(paths) => void deleteChatEntries(paths)}
          selection={chatSelection}
          onVisibleRows={setChatRows}
          // 行右键走与另外四条清单**同一个**构建器：会话归档就是库里的一篇 Markdown，
          // 同一条笔记在这儿与在文件树里能做的事不该分叉。这一格此前自己拼了一份菜单。
          onItemContextMenu={(event, entry, targets) =>
            openArchivedNoteMenu(event, entry.path, {
              paths: targets,
              // 两项资格都由 Rust 下发（`ChatHistoryDto`）：它们是**类型级**的，故挂在
              // 信封上——前端仍然不自己拼「对话是资料所以可蒸馏」这句话。
              distillable: chatController.history.distillable,
              // 「汇总为一篇」与另外四条清单同一条规则：条数区间来自 Rust，前端不写
              // 第二份数字（"至少两条"由共享构建器的多选判据本身兑现）。
              mergeable:
                chatController.history.mergeable &&
                targets.length >= chatController.history.mergeBounds.min &&
                targets.length <= chatController.history.mergeBounds.max,
              folders: chatFolders,
              onRenamed: () => void chatController.history.refresh(),
              confirm: {
                title: tChat("history.delete_title"),
                description: deleteConsequence,
                confirmLabel: tCommon("action.delete"),
              },
              onDeleted: () => void chatController.history.refresh(),
            })
          }
          folders={chatFolders}
        />
      ),
      "knowledge/tags": () => (
        <TagTreePanel
          controller={tagController}
          focusTag={tagsFocus}
          onSearchTag={openTagSearch}
          onOpenTopic={openNoteInPanes}
        />
      ),
      "knowledge/network": () => <GraphSidebar controller={graphController} />,
      // 主题页分区 = 侧栏目录 + 主区两态（选中页 = 只读阅读视图，未选中 = 库级工作台，
      // ADR-096 ② / TDD-07 §4.1d / PRD-07-024a 2026-08-31 修订）：
      // 与「侧栏承载列表、主区呈现内容」的全应用形态对齐。
      "knowledge/topics": () => (
        <TopicsView
          onOpen={openNoteInPanes}
          selectedPath={topicSelection}
          onRowMenu={openTopicRowMenu}
          onOpenHome={() => setTopicSelection(null)}
        />
      ),
      settings: () => <SettingsNav controller={settingsController} />,
    };
    return renderers[sidebarKeyOf(location)]();
  }

  return (
    /* 常驻播放宿主（ADR-118）在**空间条件渲染之外**：媒体元素与播放状态归它，切空间只卸
       掉回放视图那层投影，声音继续；离开回放视图时它给出「正在播放」条（PRD-04-064）。 */
    <PlaybackProvider>
    <div className="luna-workspace" data-media-drop={mediaDrop.hovering || undefined}>
      {/* AI 标签重扫确认卡：Shell 层常驻宿主（PRD-06-035c）——定向重扫从任意目的地的
          文件夹右键唤起（`GRAPH_RESCAN_EVENT`），不必先切到图谱。 */}
      <GraphRescanDialog controller={graphController} />
      {/* 批量整理知识笔记（PRD-06-044）：同一常驻宿主模式——任意清单的文件夹右键
          经 `DISTILL_BATCH_EVENT` 唤起，进行中进度也在此常驻（切目的地不丢）。 */}
      <DistillBatchDialog controller={distillBatch} />
      {/* 知识体检报告：同一常驻宿主模式，经 `luna:checkup` 唤起（PRD-16-030/032）。
          它必须在 Shell 层——体检卡的动作会把用户带去笔记/批量确认卡，报告自己得先关掉。 */}
      <CheckupDialog />
      {/* 新建主题页的对话框（整理中 → 逐条预览 → 确认发布，PRD-16-016/017 ADR-134 修订）。 */}
      <TopicDraftHost />
      <DistillPreviewDialog controller={distillPreview} />
      {/* 启动概览弹窗（仪表盘的新形态，PRD-07-057 二次修订）：每次运行呈现一次，
          关闭即落在笔记。内容原封复用 DashboardView（继续上次 / 队列态 / 引擎 / 用量）。 */}
      {dashboardOpen && (
        <div className="luna-overlay" role="presentation" onClick={() => setDashboardOpen(false)}>
          <div
            ref={overviewRef}
            className="luna-dialog luna-dialog--panel luna-dashboard-overlay"
            role="dialog"
            aria-modal="true"
            aria-label={t("tabs.overview")}
            onClick={(event) => event.stopPropagation()}
          >
            <header className="luna-dashboard-overlay__header">
              <strong>{t("tabs.overview")}</strong>
              <IconButton
                label={tCommon("action.close")}
                icon={<CloseIcon size={14} />}
                onClick={() => setDashboardOpen(false)}
              />
            </header>
            <div className="luna-dashboard-overlay__body">
              <DashboardView
                onDismiss={() => setDashboardOpen(false)}
                createActions={createActionContext}
              />
            </div>
            <footer className="luna-dashboard-overlay__footer">
              {/* 正向语义的勾选（「启动时显示」而非「不再显示」），取消即写设置
                  （改动即时保存惯例）；设置 › 外观里是同一个键、同一句说明。
                  受控读真实值而不是 `defaultChecked`：写死一个默认值等于界面自己编了
                  一份状态——此刻恰好等价（弹出来说明它是开的），下一次改了触发条件就不是。 */}
              <label className="luna-dashboard-overlay__toggle">
                <input
                  type="checkbox"
                  checked={showOnLaunch}
                  onChange={(event) => {
                    const value = event.target.checked;
                    setShowOnLaunch(value);
                    void commands.setSetting(LAUNCH_OVERVIEW_KEY, { kind: "bool", value });
                  }}
                />
                <span>{t("dashboard.showOnLaunch")}</span>
              </label>
            </footer>
          </div>
        </div>
      )}
      <VaultSwitchDialog controller={vaultSwitch} />
      <VaultManagerDialog
        open={vaultManagerOpen}
        onClose={() => setVaultManagerOpen(false)}
        onOpenVault={(path, name) => vaultSwitch.request(path, name)}
        // 「新建库 / 打开文件夹为库」不经 useVaultSwitch（Rust 侧 handover_and_bind 自带
        // 全套护栏），但换库后的前端收尾是同一件事：整体重挂载。
        onSwitched={() => onVaultSwitched?.()}
      />
      {/* 批量入队的「已加入 N 件」回执已并入通知 toast 栈（Rust `notify_batch_queued`，
          PRD-06-051 ⑩ 修订）：按分档时长自动消散、动作直达队列。这里只剩两种**必须
          留着让人读完**的情形：整批没发出去，或有篇目被挡下（要说得出是哪几篇、为什么）。
          初版在这儿常驻一条自己的批量进度条，与任务面板讲的是同一件事的两种说法。 */}
      {/* 整批一件都没发出去（库没开、空选择这类命令层拒绝）：说清楚"这一批压根没开始"，
          而不是伪装成某一篇的拒绝理由。 */}
      {distillBatch.outcomeError !== null && (
        <div className="luna-distill-batch-bar" role="status" aria-live="polite">
          <span>{vaultErrorText(distillBatch.outcomeError)}</span>
          <button
            type="button"
            className="luna-btn luna-btn--secondary"
            onClick={distillBatch.dismissQueued}
          >
            {tCommon("action.close")}
          </button>
        </div>
      )}
      {distillBatch.outcome !== null && (
        <div
          className="luna-distill-batch-bar"
          // 这条只在有被挡下的篇目时出现（hook 只保留那种结果），故恒为竖排小卡。
          data-detailed="true"
          role="status"
          aria-live="polite"
        >
          {/* 被挡下的那几篇必须**说得出是哪些、为什么**：只报入队数的话，50 篇里挡下
              47 篇时用户拿回的是一句「已加入 3 件」，另外 47 篇去哪了没有任何地方回答。
              理由由 Rust 逐篇本地化好（TDD-07 §2.1），这里只剥掉给机器看的那截前缀。 */}
          {distillBatch.outcome.rejected.length > 0 && (
            <ul className="luna-distill-batch-bar__rejected">
              <li className="luna-distill-batch-bar__rejected-head">
                {t("distill.batchRejected", {
                  count: distillBatch.outcome.rejected.length,
                })}
              </li>
              {distillBatch.outcome.rejected.map((item) => (
                <li key={item.note_rel}>
                  <strong>{noteStem(item.note_rel)}</strong>
                  <span>{withoutErrorCode(item.reason)}</span>
                </li>
              ))}
            </ul>
          )}
          <button
            type="button"
            className="luna-btn luna-btn--secondary"
            onClick={distillBatch.dismissQueued}
          >
            {tCommon("action.close")}
          </button>
        </div>
      )}
      <QuickSwitcher
        open={quickSwitcherOpen}
        onClose={() => {
          setQuickSwitcherOpen(false);
        }}
        onOpenPath={(path) => {
          openNoteInPanes(path);
        }}
        onOpenInSplit={(path) => {
          openNoteInSplit(path);
          go("notes");
        }}
      />
      <CommandPalette
        open={commandPaletteOpen}
        onClose={() => {
          setCommandPaletteOpen(false);
        }}
        context={commandContext}
        bindings={shortcutHints}
        hasActiveEditor={
          hasEditorSurface(location) &&
          (activeTabOf(activePaneOf(panesState))?.path?.endsWith(".md") ?? false)
        }
        unavailableSpaces={unavailableSpaces}
        hasCreatorProject={isAt("creator") && creatorView.view === "project"}
      />
      {newProjectDialog ? (
        <NewProjectDialog
          busy={newProjectDialog.busy}
          error={newProjectDialog.error}
          onCreate={(name, options) => void submitNewProject(name, options)}
          onClose={() => setNewProjectDialog(null)}
        />
      ) : null}
      <TemplatePicker
        open={templatePickerOpen}
        onClose={() => setTemplatePickerOpen(false)}
        onCreated={(path) => {
          openNoteInPanes(path);
          setAutoFocusTitle(true);
        }}
        onError={(error) => setStatus(error)}
      />
      {/* 「从链接导入」（PRD-05-006）：只在打开时挂载——工具探测与剪贴板读取都发生在
          mount，常挂会让每次进工作区都白跑一遍子进程。 */}
      {urlImportOpen && linkImportEnabled && (
        <UrlImportDialog
          onClose={() => setUrlImportOpen(false)}
          onQueued={onUrlQueued}
          onOpenSettings={(route) => {
            setUrlImportOpen(false);
            openSettingsRoute(route);
          }}
        />
      )}
      {/* 概览在场时它自己带着同一行（PRD-07-065 同一事实同屏只说一次）。 */}
      {dashboardOpen ? null : <SetupBanner setup={setupStatus} />}
      <header className="luna-workspace__topbar">
        <div className="luna-workspace__topbar-left">
          <IconButton
            className="luna-sidebar-toggle"
            label={sidebarCollapsed ? t("main.expandSidebar") : t("main.collapseSidebar")}
            icon={sidebarCollapsed ? <SidebarOpenIcon /> : <SidebarCloseIcon />}
            aria-pressed={sidebarVisible}
            disabled={!hasSidebar(space)}
            onClick={() => setSidebarCollapsed((collapsed) => !collapsed)}
          />
          <div
            className="luna-workspace__topbar-breadcrumbs"
            aria-label={t("main.breadcrumbsLabel")}
          >
            {breadcrumbs.map((crumb, idx) => (
              <span key={idx} className="luna-workspace__topbar-breadcrumb-item">
                {idx > 0 && (
                  <ChevronRightIcon
                    size={12}
                    className="luna-workspace__topbar-breadcrumb-separator"
                  />
                )}
                <span>{crumb}</span>
              </span>
            ))}
          </div>
        </div>
        <div className="luna-workspace__topbar-right">
          {/* 队列入口**常驻**（PRD-07-055）：此前只在
              `processingQueue.length > 0` 时渲染，于是空闲时「现在什么在处理」这条状态
              分流**没有入口**——用户想确认"是不是卡住了"只能靠等。空队列时面板照开，
              显示它自带的空态（`processing.empty`）。 */}
          <ProcessingQueueEntry
            open={queueOpen}
            setOpen={setQueueOpen}
            visit={queueVisit}
            onSelectNote={(noteRel) => {
              setQueueOpen(false);
              // 目的地由**这篇是什么**决定，且穷尽全部产出形态——此前这里是
              // 「transcript 否则一律录音」的二分（队列只有转写任务时代的遗留）：
              // 归入/合成任务完成后点它会跳去录音界面（真机第十轮）。
              // 知识产物（主题页/知识地图）走唯一路由闸口进知识库阅读视图。
              if (routeIfKnowledgeProduct(noteRel)) {
                return;
              }
              // 其余按笔记类型分流，直接问 Rust（ADR-003：前端不从路径反推类型；
              // 也不拿"它在不在某个列表里"当判据——分页后那只是已加载的那几页）。
              void commands.readNote(noteRel).then((result) => {
                const kind = result.status === "ok" ? result.data.meta.note_type : "note";
                if (kind === "transcript") {
                  setSelectedTranscribePath(noteRel);
                  go("transcripts");
                } else if (kind === "recording") {
                  setSelectedRecordingPath(noteRel);
                  go("recordings");
                } else {
                  // 普通笔记 / 速记 / 对话存档：打开那篇文档本身。
                  openNoteInPanes(noteRel);
                }
              });
            }}
          />
          <div className="luna-workspace__topbar-tools" aria-label={t("tools.label")}>
            {/* 「添加内容」常驻顶栏工具区最左（PRD-07-067 ②、ADR-095）：唯一跨空间创建
                入口，与一级空间正交、在任何目的地都在场，**不随侧栏收起/展开或空间切换
                迁移位置**。此前它有两个落点（侧栏页头 + 侧栏收起时的顶栏），于是同一个
                动作长期维护着两套位置与两种形态——本处即那两套合并后的唯一一份。
                **形态是带文字的紧凑按钮，不是实色横幅**：它打开的是五选一菜单而非单一
                动作，实色主按钮的层级留给各空间的上下文主动作（开始录音 / 导入文件 /
                新对话）。与右侧纯图标工具组靠**形态**区分，不靠间距。 */}
            <div className="luna-workspace__create">
              <button
                type="button"
                className="luna-btn luna-btn--secondary luna-btn--sm luna-workspace__create-btn"
                aria-haspopup="menu"
                aria-expanded={addContentOpen}
                onClick={() => setAddContentOpen((open) => !open)}
              >
                <PlusIcon size={14} />
                <span>{t("create.title")}</span>
              </button>
              {addContentOpen && (
                <AddContentPopover
                  onClose={() => setAddContentOpen(false)}
                  context={createActionContext}
                />
              )}
            </div>
            <IconButton
              className="luna-workspace__tool-button"
              label={t("tools.translate")}
              icon={<LanguagesIcon />}
              onClick={() => void showPopupTranslate()}
            />
            <IconButton
              className="luna-workspace__tool-button"
              label={t("tools.caption")}
              icon={<CaptionsIcon />}
              onClick={() => void showCaptionHud()}
            />
            {desktopLens.enabled && (
              <IconButton
                className="luna-workspace__tool-button"
                label={t(desktopLens.active ? "tools.desktopLensClose" : "tools.desktopLens")}
                icon={<CameraIcon />}
                aria-pressed={desktopLens.active}
                onClick={() => void desktopLens.toggle()}
              />
            )}
          </div>
          <IconButton
            label={t("main.searchPlaceholder")}
            icon={<SearchIcon />}
            onClick={() => setQuickSwitcherOpen(true)}
          />
          {/* 上下文面板（大纲 / 反向链接 / 外链 / 版本）**只在编辑工作面出现**：
              它的四个页签全是"当前这篇笔记的什么"，而在设置、动态、回收站这些目的地
              根本没有"当前这篇"，点开只会得到一句「未选择」——一个在哪儿都能开、
              大多数地方开了没内容的入口，本身就是噪声（真机反馈）。 */}
          {contextPanelAvailable && (
            <IconButton
              className="luna-panel-toggle"
              label={t("panel.title")}
              icon={<InfoIcon />}
              aria-pressed={!panelCollapsed}
              onClick={() => setPanelCollapsed(!panelCollapsed)}
            />
          )}
        </div>
      </header>

      <aside className="luna-workspace__rail" aria-label={t("appTitle")}>
        <div className="luna-workspace__rail-header">
          <LunaLogo size={22} className="luna-workspace__rail-logo" />
        </div>
        <WorkspacePrimaryRail
          activeSpace={space}
          layout={railLayout.items}
          forceVisible={railForceVisible}
          badges={railBadges}
          onSelect={selectSpace}
          onSetVisible={(target, visible) => void railLayout.setVisible(target, visible)}
          onReorder={(expected, ordered) => void railLayout.reorder(expected, ordered)}
        />
      </aside>

      <aside
        className="luna-workspace__sidebar"
        data-hidden={!sidebarVisible}
        aria-hidden={!sidebarVisible || undefined}
        aria-label={hasSidebar(space) ? t(spaceLabelKey(space)) : undefined}
        style={
          sidebarWidth !== null
            ? cssVariables({ "--sidebar-width": `${sidebarWidth}px` })
            : undefined
        }
      >
        {/* 设置在**无 Vault 时也要可用**——选库入口正在这一页，侧栏跟着可用。 */}
        {sidebarVisible && (vaultOpen !== false || space === "settings") ? (
          <>
            {/* 二级分区在 `sidebar-body` **之外**：body 是滚动区，把切换器放进去会
                让它在长文件树里滚出视野——而它是这一格唯一的换挡入口（PRD-07-057 ②）。 */}
            {/* 侧栏页头**不再有创建入口**（ADR-095）：「添加内容」已收敛到顶栏工具区，
                这一格让位给二级分区与列表首行。此处原是一颗全宽实色主按钮，它与其下各
                空间的上下文主动作（开始录音 / 导入文件 / 新对话）同级同色上下相邻——而
                后者才是用户已经走到这一格时真正要按的那一颗。同一侧栏内不得再出现第二颗
                同级实色主按钮。 */}
            <WorkspaceSectionNav
              space={space}
              activeSection={section}
              badges={sectionBadges}
              onSelect={(to) => {
                // 已在「主题页」再点一次 = 回到首页（工作台）：与活动栏「再点当前图标」
                // 收起侧栏同一直觉，也是选中一页后回去的第二条路。
                if (space === "knowledge" && to === "topics" && section === "topics") {
                  setTopicSelection(null);
                }
                go(space, to);
              }}
            />
            <div className="luna-workspace__sidebar-body">{renderSidebar()}</div>
            <div
              className="luna-sidebar-resizer"
              role="separator"
              aria-orientation="vertical"
              aria-label={t("sidebar.resize")}
              title={t("sidebar.resize")}
              onMouseDown={startSidebarResize}
              onDoubleClick={() => setSidebarWidth(null)}
            />
          </>
        ) : null}
      </aside>

      {/* 主区的表面类型由注册表声明（PRD-07-071）：`document` = 一张纸，`canvas` = 透明
          画布。判据不在这里，也不看"它长得像什么"——加一个空间时 `SpaceSpec` 强制要求
          声明它。 */}
      <main className="luna-workspace__main" data-surface={surfaceOf(space)}>
        {/* 没有 Vault 就没有内容可谈——除了设置（选库入口正在那一页）与 AI 对话
            （临时会话不落库照样可用）。判据按一级空间取，二级不改变这件事。 */}
        {space !== "settings" && space !== "chat" && vaultOpen === false && (
            /* 这里此前是全应用**唯一**一颗没有任何类名的按钮——渲染成浏览器默认控件，
             而"整体 UI 没适配"正是用户报过的那件事。整块改用空态原语。
             两种空态（PRD-14-062）：「上次的库找不到了」给原路径 + 重新定位；「没有打开的库」
             给三条出路（新建 / 打开文件夹 / 下载云端库）——换账户后新账户一个库都没打开过，就
             停在这里（ADR-151 修订 2026-10-15「续二」）。外置盘拔了、目录被删是常见处境，不能
             折叠成一句「尚未选择」。 */
            <EmptyState
              icon={<HardDriveIcon size={24} />}
              title={
                missingRoot
                  ? t(missingInCloud ? "main.vaultInCloudTitle" : "main.vaultMissingTitle")
                  : t("nav.vault")
              }
              description={
                missingRoot
                  ? t(missingInCloud ? "main.vaultInCloud" : "main.vaultMissing", {
                      path: missingRoot,
                    })
                  : t("main.noVault")
              }
              action={
                missingRoot ? (
                  <>
                    {missingInCloud && (
                      <button
                        type="button"
                        className="luna-btn luna-btn--primary"
                        onClick={() => {
                          void retryVaultInCloud(missingRoot);
                        }}
                      >
                        <ArrowClockwiseIcon size={14} />
                        <span>{t("main.retryVaultInCloud")}</span>
                      </button>
                    )}
                    <button
                      type="button"
                      className={
                        missingInCloud ? "luna-btn luna-btn--secondary" : "luna-btn luna-btn--primary"
                      }
                      onClick={() => {
                        void relocateVault();
                      }}
                    >
                      <FolderIcon size={14} />
                      <span>{t("main.relocateVault")}</span>
                    </button>
                    <button
                      type="button"
                      className="luna-btn luna-btn--secondary"
                      onClick={() => {
                        go("settings");
                      }}
                    >
                      <SettingsIcon size={14} />
                      <span>{t("main.chooseAnotherVault")}</span>
                    </button>
                  </>
                ) : (
                  <VaultEmptyActions
                    onSwitched={() => onVaultSwitched?.()}
                    onOpenVault={(path, name) => vaultSwitch.request(path, name)}
                  />
                )
              }
            />
          )}

        {hasEditorSurface(location) && vaultOpen !== false && (
          <div className="luna-panes">
            {panesState.panes.map((pane) => {
              const tab = activeTabOf(pane);
              const paneActive = pane.id === panesState.activePaneId;
              // 非 md 附件（图片/音视频/二进制）走预览视图；笔记/文件夹/空标签走编辑器（PRD-06-007）。
              const assetKind = classifyAssetTarget(tab?.path ?? null);
              return (
                <section
                  key={pane.id}
                  className="luna-pane"
                  data-active={paneActive || undefined}
                  aria-label={t("tabsbar.paneLabel")}
                  onMouseDownCapture={() => {
                    if (!paneActive) {
                      setPanesStateRaw((state) => setActivePane(state, pane.id));
                    }
                  }}
                >
                  <NoteTabsBar
                    pane={pane}
                    paneActive={paneActive}
                    activeTabDirty={paneMetrics[pane.id]?.dirty ?? false}
                    canSplit={panesState.panes.length < MAX_PANES}
                    canClosePane={panesState.panes.length > 1}
                    canPopOut={Boolean(tab?.path && tab.path.endsWith(".md"))}
                    onActivateTab={(tabId) => {
                      setPanesState((state) => activateTab(state, pane.id, tabId));
                    }}
                    onKeepTab={(tabId) => {
                      setPanesState((state) => keepTab(state, pane.id, tabId));
                    }}
                    onCloseTab={(tabId) => {
                      setPanesState((state) => closeTab(state, pane.id, tabId));
                    }}
                    onTogglePin={(tabId) => {
                      setPanesState((state) => togglePinTab(state, pane.id, tabId));
                    }}
                    onCloseOtherTabs={(tabId) => {
                      setPanesState((state) => closeOtherTabs(state, pane.id, tabId));
                    }}
                    onCloseRightTabs={(tabId) => {
                      setPanesState((state) => closeRightTabs(state, pane.id, tabId));
                    }}
                    onNewTab={() => {
                      setPanesState((state) => openEmptyTab(state, pane.id));
                    }}
                    onGoBack={() => {
                      setPanesState((state) => goBack(state, pane.id));
                    }}
                    onGoForward={() => {
                      setPanesState((state) => goForward(state, pane.id));
                    }}
                    onSplit={() => {
                      setPanesState((state) => splitPane(state, pane.id));
                    }}
                    onClosePane={() => {
                      setPaneMetrics((current) => {
                        const next = { ...current };
                        delete next[pane.id];
                        return next;
                      });
                      setPanesState((state) => closePane(state, pane.id));
                    }}
                    onPopOut={() => {
                      if (tab?.path && tab.path.endsWith(".md")) {
                        void openNoteWindow(tab.path, {
                          move: { paneId: pane.id, tabId: tab.id },
                        });
                      }
                    }}
                    onMoveTab={(from, to) => {
                      setPanesState((state) => moveTab(state, from, to));
                    }}
                  />
                  {/* 项目里的脚本在「笔记」里打开：只给一条「在创作中打开」（ADR-165 ⑦）。脚本时长只在
                      创作启用、这一篇是脚本时才算，故以它为判据，不另判类型。 */}
                  {!assetKind && tab?.path && paneMetrics[pane.id]?.path === tab.path && paneMetrics[pane.id]?.script ? (
                    <ScriptProjectBanner path={tab.path} />
                  ) : null}
                  {assetKind && tab?.path ? (
                    <AssetView path={tab.path} kind={assetKind} />
                  ) : (
                    <VaultEditor
                      path={tab?.path ?? null}
                      canvasDestination={canvasDestination(location)}
                      active={paneActive}
                      viewMode={tab?.mode ?? "live"}
                      onViewModeChange={(mode) => {
                        if (tab) {
                          setPanesStateRaw((state) => setTabMode(state, pane.id, tab.id, mode));
                        }
                      }}
                      autoFocusTitle={paneActive && autoFocusTitle}
                      onTitleFocused={() => setAutoFocusTitle(false)}
                      onOpenPath={(openedPath) => {
                        // 编辑器内链点击：同桌打开不切空间，但产物照样让路（ADR-096 ③）。
                        if (routeIfKnowledgeProduct(openedPath)) {
                          return;
                        }
                        setPanesState((state) => openNote(state, openedPath, { paneId: pane.id, as: intentAs }));
                        setAutoFocusTitle(false);
                      }}
                      onOpenTag={openTagView}
                      onSavedPath={(savedPath, previousPath) => {
                        setPanesStateRaw((state) =>
                          previousPath
                            ? remapPath(state, previousPath, savedPath)
                            : openNote(state, savedPath, { paneId: pane.id }),
                        );
                        void refreshVault();
                      }}
                      onStatus={setStatus}
                      onEditorMetrics={(metrics) => {
                        // 开始编辑 = 要留着它：预览标签转常驻（PRD-07-035）。
                        if (metrics.dirty && tab?.preview) {
                          setPanesState((state) => keepTab(state, pane.id, tab.id));
                        }
                        setPaneMetrics((current) => {
                          const existing = current[pane.id];
                          if (
                            existing &&
                            existing.dirty === metrics.dirty &&
                            existing.wordCount === metrics.wordCount && existing.selectedWords === metrics.selectedWords && existing.path === metrics.path && existing.outline === metrics.outline && existing.wordGoal === metrics.wordGoal && existing.script === metrics.script
                          ) {
                            return current;
                          }
                          return { ...current, [pane.id]: metrics };
                        });
                      }}
                      folderStats={folderStatsFor(tab?.path ?? null)}
                    />
                  )}
                  {paneActive && externalRenamePrompt !== null && (
                    <ExternalRenameBar
                      prompt={externalRenamePrompt}
                      onRepaired={(renames) =>
                        setPanesState((state) =>
                          renames.reduce((next, rename) => remapPath(next, rename.from, rename.to), state),
                        )
                      }
                      onStatus={setStatus}
                    />
                  )}
                  {paneActive && obsidianSuggestion !== null && (
                    <ObsidianSuggestionBar
                      suggestion={obsidianSuggestion}
                      onSettled={settleObsidianSuggestion}
                      onStatus={setStatus}
                    />
                  )}
                  {paneActive && relocationReceipt !== null && (
                    <RelocationReceiptBar
                      receipt={relocationReceipt}
                      onDismiss={dismissRelocationReceipt}
                      onMovedBack={(from, to) => setPanesState((state) => remapPath(state, from, to))}
                      onStatus={setStatus}
                    />
                  )}
                </section>
              );
            })}
          </div>
        )}

        {isAt("quick_notes") && vaultOpen !== false && (
          <div className="luna-quick-note-pane">
            {/* 撤销条（PRD-06-058 C）：转正是一步可逆动作，撤销就该在**视线所在处**。
                转正之后这块主区正好空出来，条子挂在最上面一眼就能看到；此前它只长在
                侧栏里，从这颗按钮转正的人根本看不到它。 */}
            {undoSettlePath !== null && (
              <div className="luna-quick-note-pane__undo" role="status">
                <span>{t("draft.settledNotice")}</span>
                <button
                  type="button"
                  className="luna-btn luna-btn--secondary luna-btn--sm"
                  onClick={() => {
                    const path = undoSettlePath;
                    setUndoSettlePath(null);
                    void demoteQuickNote(path);
                  }}
                >
                  {t("draft.undo")}
                </button>
              </div>
            )}
            {/* 「整理」栏（PRD-06-058 C）：选中一条时常驻，与侧栏行 hover 动作同源；
                未选中时主区是空白捕获画布，栏不出现。 */}
            {selectedDraftPath !== null && (
              <QuickNoteActions
                onSettle={() => void settleDraft(selectedDraftPath)}
                onDistill={() => requestDistillNotes([selectedDraftPath])}
                onOpenInNotes={() => openNoteInPanes(selectedDraftPath)}
                onDelete={() => {
                  const path = selectedDraftPath;
                  void deleteArchivedNotes(
                    [path],
                    {
                      title: t("main.deleteQuickNoteTitle", { path }),
                      description: deleteConsequence,
                      confirmLabel: tCommon("action.delete"),
                    },
                    () => setSelectedPath(null),
                  );
                }}
              />
            )}
            <VaultEditor
              path={selectedDraftPath}
              canvasDestination={canvasDestination(location)}
              autoFocusTitle={autoFocusTitle}
              onTitleFocused={() => setAutoFocusTitle(false)}
              onOpenPath={(openedPath) => {
                openNoteInPanes(openedPath);
                setAutoFocusTitle(false);
              }}
              onSavedPath={(savedPath) => {
                setSelectedPathRaw(savedPath);
                void refreshVault();
              }}
              onStatus={setStatus}
              onEditorMetrics={setEditorMetrics}
            />
          </div>
        )}

        {/* 主题页分区的主区两态（PRD-07-024a 2026-08-31 修订）：选中页 = 只读阅读视图；
            未选中 = 库级工作台（原「概览」分区的内容，分区已退役）。 */}
        {isAt("knowledge", "topics") &&
          vaultOpen !== false &&
          (topicSelection !== null ? (
            <KnowledgeReadingView
              path={topicSelection}
              onOpenPath={openNoteInPanes}
              onOpenTag={openTagView}
              onStatus={setStatus}
              onBack={() => setTopicSelection(null)}
            />
          ) : (
            <KnowledgeWorkbench onCreateNote={() => void createNewNote()} />
          ))}

        {isAt("knowledge", "tags") && vaultOpen !== false && (
          <TagNotesView
            controller={tagController}
            selectedPath={selectedPath}
            onSearchTag={openTagSearch}
            onOpenTopic={openNoteInPanes}
            onSelect={(path) => {
              openNoteInPanes(path, { as: browseAs });
            }}
            onItemContextMenu={(event, item, paths) =>
              openArchivedNoteMenu(event, item.path, {
                paths,
                confirm: {
                  title: t("main.deleteNoteTitle"),
                  description: deleteConsequence,
                  confirmLabel: tCommon("action.delete"),
                },
                onDeleted: () => setSelectedPath(null),
                onRenamed: (to) => setSelectedPath(to),
              })
            }
          />
        )}

        {isAt("knowledge", "network") && vaultOpen !== false && (
          <GraphView
            controller={graphController}
            focus={graphFocus}
            onOpenNote={(path) => {
              openNoteInPanes(path, { as: browseAs });
            }}
            onOpenTag={openTagView}
            onStatus={setStatus}
          />
        )}

        {isAt("recordings") &&
          vaultOpen !== false &&
          (selectedRecordingPath ? (
            <SyncPlaybackView noteRel={selectedRecordingPath} />
          ) : /* 主区两态各说各的：库里一条录音都没有 = 去开始录一段（给动作，不是把侧栏
               那句话再念一遍）；有录音但没选 = 从左边挑一条。此前两态共用一句裸 `<p>`，
               还与侧栏空态重复。 */
          // 「一条都没有」只看库里有没有，不看筛选后剩几条：筛选查无时库里仍有录音，主区该说
          // 「从左边挑一条」，而不是「还没有录音 · 开始录音」（真机 2026-10-06）。
          recordingNotes.length === 0 && recordingNeedle.length === 0 ? (
            /* 主区不复述侧栏那句「还没有录音」（同屏同一句话两遍，PRD-07-065；真机 2026-10-06
               两处标题、说明、按钮一模一样）：这里说录音能给你什么，并给录音设置的入口，与转写
               空库的主区同一个思路。 */
            <EmptyState
              icon={<DiscIcon size={24} />}
              title={t("capture.recordingLandingTitle")}
              description={t("capture.recordingLandingBody")}
              action={
                <>
                  <button
                    type="button"
                    className="luna-btn luna-btn--primary"
                    onClick={() => void showRecordingHud()}
                  >
                    <DiscIcon size={14} />
                    <span>{t("capture.startRecording")}</span>
                  </button>
                  <button
                    type="button"
                    className="luna-btn luna-btn--secondary"
                    onClick={() => openSettingsRoute("settings://recording")}
                  >
                    <SettingsIcon size={14} />
                    <span>{t("capture.recordingSettings")}</span>
                  </button>
                </>
              }
            />
          ) : (
            <EmptyState icon={<DiscIcon size={24} />} title={t("main.recordingSelectHint")} />
          ))}

        {isAt("transcripts") && vaultOpen !== false && (
          <TranscribeView
            selectedPath={selectedTranscribePath}
            hasNotes={transcribeLibraryHasNotes}
            renderPlayback={(noteRel) => <SyncPlaybackView noteRel={noteRel} />}
            importer={transcribeImport}
            onImport={() => void startImport()}
            onImportUrl={linkImportEnabled ? () => setUrlImportOpen(true) : undefined}
            onOpenSettings={() => openSettingsRoute("settings://recording/media")}
          />
        )}

        {isAt("trash") && vaultOpen !== false && (
          <TrashView
            entries={trashEntries}
            mode={deleteMode}
            onRestored={(path) => openNoteInPanes(path)}
            onChanged={() => void refreshVault()}
            onStatus={(message) => setStatus(message)}
          />
        )}

        {isAt("chat") && <ChatView controller={chatController} />}

        {isAt("creator") && vaultOpen !== false && creatorView.view === "project" && (
          <ProjectWorkspace
            key={creatorView.home}
            home={creatorView.home}
            pane={creatorView.pane}
            onPane={(pane) => setCreatorView({ view: "project", home: creatorView.home, pane })}
            onBoard={() => setCreatorView({ view: "board" })}
            renderEditor={renderCreatorEditor}
            renderPublish={({ home }) => (
              <PublishView
                home={home}
                onImportQueued={() => setQueueOpen(true)}
                onOpenNote={(path) => setCreatorView({ view: "project", home, pane: { tab: "doc", path } })}
              />
            )}
            onImportQueued={() => setQueueOpen(true)}
            onOpenInNotes={(path) => openNoteInPanes(path)}
            onActiveDocument={setCreatorDocumentPath}
            scriptOutline={editorMetrics.path === creatorDocumentPath ? editorMetrics.script : null}
            createActions={createActionContext}
            projectNav={creatorProjectNav}
            onItemMenu={openCreatorNoteMenu}
          />
        )}

        {isAt("creator") && vaultOpen !== false && creatorView.view === "note" && (
          <CreatorNotePage
            key={creatorView.path}
            path={creatorView.path}
            onBack={() => setCreatorView({ view: "swipes" })}
            renderEditor={renderCreatorEditor}
            onOpenPath={(path) => setCreatorView({ view: "note", path })}
            onOpenInNotes={(path) => openNoteInPanes(path)}
            onActiveDocument={setCreatorDocumentPath}
            onMore={openCreatorNoteMenu}
          />
        )}

        {isAt("creator") && vaultOpen !== false && creatorView.view === "swipes" && (
          <SwipeLibrary
            onBack={() => setCreatorView({ view: "board" })}
            onOpenNote={(path) => setCreatorView({ view: "note", path })}
            onCreateSwipe={() => void createSwipe()}
          />
        )}

        {isAt("creator") && vaultOpen !== false && creatorView.view === "board" && (
          <CreatorBoard
            state={creatorBoard}
            onCreateProject={() => void createCreatorProject()}
            onOpenColumnSettings={() => openSettingsRoute("settings://general/creator.board_stages")}
            onOpenProject={(card) =>
              setCreatorView({ view: "project", home: card.path, pane: { tab: "overview" } })
            }
            projectNav={creatorProjectNav}
          />
        )}

        {isAt("teleprompter") && vaultOpen !== false && <TeleprompterHome />}

        {isAt("apps") && <AppsView selected={appsSelection} />}

        {isAt("settings") && (
          <SettingsView
            controller={settingsController}
            pagePanels={SETTINGS_PAGE_PANELS}
            captionPreview={<CaptionSettingsPreview specs={settingsController.allSpecs} />}
            focus={settingsFocus}
          />
        )}
      </main>

      <aside
        className="luna-workspace__panel"
        data-collapsed={panelCollapsed || !contextPanelAvailable}
        aria-hidden={panelCollapsed || !contextPanelAvailable}
        ref={(element) => { if (element) element.inert = panelCollapsed || !contextPanelAvailable; }}
        aria-label={t("panel.tabsLabel")}
      >
        <div className="luna-workspace__panel-header">
          <h2>{t("panel.title")}</h2>
          {/* 关闭钮的可访问名是「关闭」，不是它所在面板的名字——读屏念出「上下文，按钮」
              说不清按下去会发生什么。字形也统一为 `CloseIcon`（全应用此前有 ✕ / &times; /
              × / 文字「关闭」四种写法）。 */}
          <IconButton
            className="luna-panel-toggle"
            label={tCommon("action.close")}
            icon={<CloseIcon size={14} />}
            onClick={() => setPanelCollapsed(true)}
          />
        </div>
        <div className="luna-workspace__panel-body">
          <ContextPanel
            path={contextNotePath}
            outline={editorCanvasVisible && activeMetrics.path === contextNotePath ? activeMetrics.outline : undefined}
            defaultTab={isAt("recordings") || isAt("transcripts") || isAt("chat") ? "info" : "outline"}
            onOpenPath={(path) => openNoteInPanes(path, { as: browseAs })}
            onOpenTag={openTagView}
            onOpenGraph={(path) => {
              setGraphFocus((current) => ({ path, nonce: (current?.nonce ?? 0) + 1 }));
              go("knowledge", "network");
            }}
          />
        </div>
      </aside>

      <footer className="luna-workspace__statusbar">
        <div className="luna-workspace__statusbar-left">
          {/* 库切换器（PRD-07-064）：状态栏最左，Obsidian 同位——「我在哪个库」
              与「就绪 / 字数」同级，是工作区的基础可观测信息。 */}
          <VaultSwitcher
            onManage={() => setVaultManagerOpen(true)}
            onSwitch={(path) => {
              const name = path.split("/").filter(Boolean).pop() ?? path;
              vaultSwitch.request(path, name);
            }}
            switchingTo={vaultSwitch.switchingTo}
            onError={setStatus}
          />
          {/* 录音中的回程（B2）：点它回到录音面板，而不是录音列表——面板才是现场。
              每秒走的计时只属于这颗按钮（根只持有录音状态的转移）。 */}
          <StatusBarRecording indicator={recordingIndicator} />
          <StatusBarMessage status={status} />
        </div>

        <div className="luna-workspace__statusbar-right">
          {editorCanvasVisible && (!activeDocumentPath || activeDocumentPath.endsWith(".md")) && !activeMetrics.loadFailed && (
            <>
              {/* 字数目标（W6b）：设了目标就报「已写 / 目标」，达成后换一种说法并着强调色。 */}
              <span
                className="luna-workspace__statusbar-words"
                data-goal-reached={
                  (activeMetrics.selectedWords === null &&
                    activeMetrics.wordGoal !== null &&
                    activeMetrics.wordCount >= activeMetrics.wordGoal) ||
                  undefined
                }
              >
                {activeMetrics.selectedWords !== null
                  ? t("statusbar.wordsSelected", { selected: activeMetrics.selectedWords, count: activeMetrics.wordCount })
                  : activeMetrics.wordGoal === null
                    ? t("statusbar.words", { count: activeMetrics.wordCount })
                    : t(activeMetrics.wordCount >= activeMetrics.wordGoal ? "statusbar.wordsGoalReached" : "statusbar.wordsGoal", {
                        count: activeMetrics.wordCount,
                        goal: activeMetrics.wordGoal,
                      })}
              </span>
              {/* 视频脚本的口播时长（PRD-19-012）：Rust 估算，未按本人语速校准时标「约」。 */}
              {activeMetrics.script ? (
                <span className="luna-workspace__statusbar-item">
                  {t(activeMetrics.script.calibrated ? "statusbar.scriptDuration" : "statusbar.scriptDurationApprox", {
                    duration: formatClock(activeMetrics.script.total_seconds * 1000),
                  })}
                </span>
              ) : null}
              <span className="luna-workspace__statusbar-item">
                <span className="luna-dirty-dot" data-dirty={activeMetrics.dirty} />
                {activeMetrics.dirty ? t("statusbar.dirty") : t(activeDocumentPath ? "statusbar.saved" : "statusbar.draft")}
              </span>
            </>
          )}
          {/* 「Tab 键移动焦点」开着时常驻指示，点按即关（VS Code 状态栏同法）：开关会改变整个编辑器
              的 Tab 行为，看不见就会被当成「Tab 坏了」。 */}
          {tabFocusMode && (
            <button
              type="button"
              className="luna-workspace__statusbar-mode-btn"
              aria-pressed="true"
              title={t("statusbar.tabFocusTitle")}
              onClick={() => setTabFocusMode(false)}
            >
              {t("statusbar.tabFocus")}
            </button>
          )}
          {/* 底栏上自带订阅的指示（CPU / GPU、算力、同步、通知）各自订各自的广播：
              一拍只重渲染那一颗，不经工作区根（2026-10-15 真机卡顿）。 */}
          <StatusBarResource />
          <StatusBarCompute />
          <StatusBarSync />
          <StatusBarNotifications />
        </div>
      </footer>
    </div>
    </PlaybackProvider>
  );
}
