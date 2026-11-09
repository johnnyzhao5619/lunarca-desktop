// AI 面板容器（PRD-06-020/021）：摆放 + 三种内容态——指令框（`AiComposer`）、结果区
// （`AiResultPanel`）、库内搜索结果——外加整篇作用域的关系标签动作。
//
// 定位由面板**自己**量自己的尺寸（`useBoxSize`）再算：下方优先、放不下翻上方，最大高度
// 取上下两侧空间的较大者（先定上限再量高度，不会在「翻上去 → 变矮 → 又翻下来」之间抖）。
// 宿主只给选区包围盒与编辑器尺寸；整篇模式无锚点，顶部停靠（CSS 默认位）。
//
// 前端零业务：动作清单按作用域摆放；建议落地方式、执行位置、发送字数都是 Rust 随流首
// 事实下发的投影（`use-floating-menu.ts`）。

import { useEffect, useMemo, useRef, useState, type MutableRefObject } from "react";
import { useTranslation } from "react-i18next";
import { cssVariables } from "../../lib/css-variables";
import { useBoxSize } from "../../hooks/use-resize-observer";
import { useAppEnabled, useNoteTypeToolsAvailable } from "../../hooks/use-apps";
import { useTemplateMarket } from "../../hooks/use-template-market";
import { APPS_TEMPLATES_ROUTE } from "../../lib/app-route";
import { goTo } from "../../lib/navigate";
import { formatShortcut } from "@lunarca/editor-core/shortcut-display";
import {
  commands,
  type FloatingMenuAction,
  type FloatingMenuLanding,
  type FloatingMenuOptionsDto,
  type FloatingMenuPendingDto,
  type FloatingMenuPreflightDto,
  type FloatingMenuScope,
  type FloatingPresetDto,
  type NoteTypeDto,
  type SearchHitDto,
} from "../../ipc/bindings";
import { FilteredList } from "../command/FilteredList";
import {
  BookmarkIcon,
  FilmIcon,
  CheckIcon,
  CloseIcon,
  GraphIcon,
  LanguagesIcon,
  PencilIcon,
  PlusIcon,
  ScrollTextIcon,
  SearchIcon,
  SparklesIcon,
  SpeakIcon,
  TableIcon,
  TagsIcon,
} from "@lunarca/editor-core/Icon";
import { sentenceAround, type DiffChange } from "./ai-diff";
import { AiComposer, type ComposerItem } from "./AiComposer";
import { AiResultPanel, orderedLandings } from "./AiResultPanel";
import {
  clampPanelPosition,
  FLOATING_MENU_GAP,
  placeFloatingMenu,
  type EditorBox,
  type SelectionBox,
} from "./floating-menu-position";
import { useFloatingMenu, type FloatingMenuContext } from "./use-floating-menu";

/** 宿主键位（⌘↩ 接受 / Esc 停止或关闭）经此 ref 转进面板——结果态只有面板知道。 */
export interface FloatingMenuControls {
  accept: () => boolean;
  escape: () => boolean;
}

export interface FloatingMenuProps {
  scope: FloatingMenuScope;
  /** 「AI 操作」开合的键位（设置真值，W3b；空 = 没绑键）。 */
  aiShortcut: string;
  /** 选区文本 / 光标前文 / 整篇正文（按作用域）。 */
  selectedText: string;
  /** 选区前后段落的原样切片（选区作用域）；其它作用域 `null`。 */
  context: FloatingMenuContext | null;
  /** 当前笔记 Vault 相对路径（整篇「总结」随之传给 Rust：录音笔记 → 会议纪要，PRD-04-054）。 */
  notePath?: string | null;
  /** 当前笔记的类型：视频脚本（`script`）多一组「脚本 ▸」改稿动作（PRD-19-013/014）。 */
  noteType?: NoteTypeDto | null;
  /** 选区 / 光标包围盒（相对编辑器容器）；`null` = 顶部停靠（整篇模式）。 */
  anchorBox: SelectionBox | null;
  editorSize: EditorBox;
  /** 打开即执行库内搜索（气泡条「相关笔记」入口）。 */
  initialSearch?: boolean;
  /** 后台跑完、等着落地的结果（PRD-06-021）：打开面板即呈现它，不再重跑。 */
  pending?: FloatingMenuPendingDto | null;
  /** 打开即开跑的动作（脚本工具栏的「AI 改稿」，ADR-165 ⑦）；只跑一次。 */
  autoRun?: FloatingMenuAction | null;
  /** `editor.floating_context` 的当前值与切换（设置项，宿主持有）。 */
  contextEnabled: boolean;
  onToggleContext: () => void;
  onLand: (text: string, landing: FloatingMenuLanding) => void;
  onOpenPath?: (path: string) => void;
  onClose: () => void;
  /** 生成/更新 AI 关系标签（PRD-06-035，整篇动作）：核心在宿主，面板只呈现结果。 */
  onGraphTags?: () => Promise<{ ok: true; tags: string[] } | { ok: false; error: string }>;
  onViewGraph?: () => void;
  controlsRef?: MutableRefObject<FloatingMenuControls | null>;
  /**
   * 从表格菜单打开（对标方案 T6）：`explain` 打开即总结这张表；`fill_column` 进入「描述这一列
   * 要填什么」的指令输入，回车即逐行填写那一列。作用域都是这张表的源码（选区）。
   */
  tableTask?: TableAiTask | null;
}

export type TableAiTask = { kind: "explain" } | { kind: "fill_column"; column: number; header: string };

type GraphTagsState =
  | { status: "idle" }
  | { status: "running" }
  | { status: "done"; tags: string[] }
  | { status: "failed"; message: string };

/** 无子菜单选择（跟随设置）。 */
const NO_OPTIONS: FloatingMenuOptionsDto = {
  target_lang: null,
  summary_style: null,
  table_column: null,
  target_seconds: null,
};

/** 「压到 N 秒」的几档目标时长（ADR-176 ⑧；Shorts 60 秒以内、口播开场常见 30 / 90 秒）。 */
const PROMPTER_FIT_SECONDS = [30, 60, 90] as const;

/** 首帧占位尺寸（layout effect 量到真实值后同帧重算）。 */
const FALLBACK_SIZE = { width: 420, height: 240 };

/** 翻译子菜单的目标语言（与设置页翻译目标语言同一张表，`luna-config::TRANSLATE_TARGET_LANGS`）。 */
const TRANSLATE_TARGET_LANGS = ["zh", "en", "ja", "ko", "fr", "de", "es", "pt", "ru", "it"] as const;

export function FloatingMenu({
  scope,
  aiShortcut,
  selectedText,
  context,
  notePath = null,
  noteType = null,
  anchorBox,
  editorSize,
  initialSearch = false,
  pending = null,
  autoRun = null,
  contextEnabled,
  onToggleContext,
  onLand,
  onOpenPath,
  onClose,
  onGraphTags,
  onViewGraph,
  controlsRef,
  tableTask = null,
}: FloatingMenuProps) {
  const { t } = useTranslation("editor");
  const scriptActionsAvailable = useNoteTypeToolsAvailable(noteType);
  // 提词器的 AI 口播动作归「提词器」应用：任何笔记都能用，应用停用时整组不出现（ADR-176 ⑧）。
  const prompterActionsAvailable = useAppEnabled("teleprompter");
  const {
    result,
    lastInstruction,
    candidateIndex,
    candidateCount,
    selectCandidate,
    searchState,
    run,
    followUp,
    stop,
    search,
    retry,
    land,
    seed,
    reset,
  } = useFloatingMenu();
  const [graphTags, setGraphTags] = useState<GraphTagsState>({ status: "idle" });
  /** 对话 / 改写能力此刻是否在服务端跑；`null` = 尚未查到。 */
  const [onServer, setOnServer] = useState<boolean | null>(null);
  /** 提交前长文预检（PRD-06-025）。 */
  const [preflight, setPreflight] = useState<FloatingMenuPreflightDto | null>(null);
  /** 用户自定义预设（PRD-06-024，随库）。 */
  const [presets, setPresets] = useState<FloatingPresetDto[]>([]);
  const { market: templateMarket } = useTemplateMarket();
  /** 一次性状态（预设已保存）。 */
  const [status, setStatus] = useState<string | null>(null);
  /** 这一轮的子菜单选择：转入后台时原样交给队列，后台跑的必须是同一件事。 */
  const [backgroundOptions, setBackgroundOptions] = useState<FloatingMenuOptionsDto>(NO_OPTIONS);
  /**
   * 用户拖到的位置（相对编辑器容器）。非 `null` 即**接管摆放**：自动摆放会跟着选区与
   * 滚动重算，那正是用户拖它的原因——他要的是「别再挡着这段正文」，而不是一个会自己
   * 跑回来的面板。
   */
  const [dragPos, setDragPos] = useState<{ x: number; y: number } | null>(null);
  const [dragging, setDragging] = useState(false);
  const panelRef = useRef<HTMLDivElement | null>(null);
  const size = useBoxSize(panelRef);

  const running = result.status === "running";
  const canUseResult = result.status === "done" && result.output.trim().length > 0;
  const resultActive = result.status !== "idle";
  const searchActive = searchState.status !== "idle";
  const graphTagsActive = graphTags.status !== "idle";
  const hasContext = context !== null && (context.before.length > 0 || context.after.length > 0);
  const toggleShortcut = aiShortcut ? formatShortcut(aiShortcut) : "";
  const acceptShortcut = formatShortcut("cmd_or_ctrl+enter");

  // 面板卸载（切笔记/关闭编辑器）时若仍在生成，停止后台任务，避免孤儿 Job 空转。
  const runningRef = useRef(false);
  runningRef.current = running;
  useEffect(
    () => () => {
      if (runningRef.current) {
        void commands.floatingMenuStop();
      }
    },
    [],
  );

  // 执行位置：与设置页「执行位置」卡片同一命令、同一判据（ADR-059）。
  useEffect(() => {
    let cancelled = false;
    void commands.computeStatus().then((status) => {
      if (cancelled || status.status !== "ok") {
        return;
      }
      const chat = status.data.routes.find((route) => route.capability === "chat");
      setOnServer(chat ? chat.location === "server" : null);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  // 后台结果：面板一打开就把它摆出来（只做一次；此后用户可能重试 / 追问，那是新候选）。
  const seededRef = useRef(false);
  useEffect(() => {
    if (pending && !seededRef.current) {
      seededRef.current = true;
      seed(pending);
    }
  }, [pending, seed]);

  // 工具栏点的是某个具体动作：面板一打开就跑它（只跑一次，此后与手点同一路径）。
  const autoRanRef = useRef(false);
  useEffect(() => {
    if (autoRun && !autoRanRef.current) {
      autoRanRef.current = true;
      runAction(autoRun);
    }
  });

  // 预检与预设都只在打开那一刻取一次：文本与库在这扇面板的生命周期内不变。
  useEffect(() => {
    let cancelled = false;
    // 数**码点**而不是 UTF-16 长度：Rust 侧的策略判据用的是 `chars().count()`，
    // 两处口径不一致时，含 emoji 的正文会出现「预检说要分段、实际没分」。
    void commands.floatingMenuPreflight([...selectedText].length, scope).then((outcome) => {
      if (!cancelled && outcome.status === "ok") {
        setPreflight(outcome.data);
      }
    });
    void commands.floatingPresetsList().then((outcome) => {
      if (!cancelled && outcome.status === "ok") {
        setPresets(outcome.data);
      }
    });
    return () => {
      cancelled = true;
    };
  }, [scope, selectedText.length]);

  function savePreset(name: string) {
    if (!lastInstruction) {
      return;
    }
    void commands.floatingPresetSave(name, lastInstruction).then((outcome) => {
      if (outcome.status === "ok") {
        setPresets(outcome.data);
        setStatus(t("floating.presets.saved"));
      }
    });
  }

  /** 校对某处变更的解释：只发含这一处的那句话，不发整段。 */
  async function explainChange(change: DiffChange): Promise<string> {
    const outcome = await commands.floatingMenuExplain(
      sentenceAround(selectedText, change.from, change.to),
      change.original,
      change.replacement,
    );
    if (outcome.status === "ok") {
      return outcome.data;
    }
    throw new Error(outcome.error);
  }

  // 「压到 N 秒」这一轮的目标与估时（ADR-176 ⑧）；身份随目标秒数变，结果面板据此重估。
  const fitSecondsTarget =
    result.action === "prompter_fit" ? backgroundOptions.target_seconds : null;
  const fitTarget = useMemo(
    () =>
      fitSecondsTarget === null
        ? null
        : {
            seconds: fitSecondsTarget,
            estimate: async (text: string) => {
              const outcome = await commands.teleprompterEstimateSeconds(text);
              return outcome.status === "ok" ? outcome.data : null;
            },
          },
    [fitSecondsTarget],
  );

  function deletePreset(id: string) {
    void commands.floatingPresetDelete(id).then((outcome) => {
      if (outcome.status === "ok") {
        setPresets(outcome.data);
      }
    });
  }

  // 只在打开那一刻按入口意图执行一次（气泡「相关笔记」→ 开即搜）；`search` 与
  // `selectedText` 都取打开那一刻的值，之后不再触发。
  const openedWithSearchRef = useRef(initialSearch);
  useEffect(() => {
    if (openedWithSearchRef.current) {
      openedWithSearchRef.current = false;
      void search(selectedText);
    }
  });

  // 表格菜单的「总结这张表」：打开那一刻执行一次（与上面的开即搜同一做法）。
  const openedWithExplainRef = useRef(tableTask?.kind === "explain");
  useEffect(() => {
    if (openedWithExplainRef.current) {
      openedWithExplainRef.current = false;
      runAction("table_explain");
    }
  });

  /**
   * 从头部开始拖。用指针捕获而不是往 window 上挂监听：捕获期间指针事件全归这个元素，
   * 拖到面板外、拖出窗口再松手都收得到 `pointerup`，不会留下一个"松了手还在跟着走"
   * 的面板。关闭钮上不起拖——否则那颗钮点不着。
   */
  function startDrag(event: React.PointerEvent<HTMLElement>) {
    if (event.button !== 0 || (event.target as HTMLElement).closest("button")) {
      return;
    }
    const panel = panelRef.current;
    const container = panel?.offsetParent as HTMLElement | null;
    if (!panel || !container) {
      return;
    }
    // 头部按下不该夺走编辑器焦点：选区高亮留着，用户才看得见自己正在对哪一段动作。
    event.preventDefault();
    const panelRect = panel.getBoundingClientRect();
    const containerRect = container.getBoundingClientRect();
    const grabX = event.clientX - panelRect.left;
    const grabY = event.clientY - panelRect.top;
    const size = { width: panelRect.width, height: panelRect.height };
    // 存成局部变量：`currentTarget` 只在派发那一刻有效，闭包里再读到的可能是 null，
    // 那样 `pointerup` 就摘不掉监听——面板会一直跟着指针走。
    const handle = event.currentTarget;
    handle.setPointerCapture(event.pointerId);
    setDragging(true);

    const onMove = (move: PointerEvent) => {
      setDragPos(
        clampPanelPosition(
          move.clientX - containerRect.left - grabX,
          move.clientY - containerRect.top - grabY,
          editorSize,
          size,
        ),
      );
    };
    const onUp = () => {
      setDragging(false);
      handle.removeEventListener("pointermove", onMove);
      handle.removeEventListener("pointerup", onUp);
      handle.removeEventListener("pointercancel", onUp);
    };
    handle.addEventListener("pointermove", onMove);
    handle.addEventListener("pointerup", onUp);
    handle.addEventListener("pointercancel", onUp);
  }

  function handleClose() {
    if (running) {
      stop();
    }
    onClose();
  }

  /** 用户已经处置这一篇的后台结果：清掉待落地槽，免得下次打开又弹一次。 */
  function clearPending() {
    if (notePath) {
      void commands.floatingMenuPendingClear(notePath);
    }
  }

  /** 面板把最终文本一起交上来（校对按接受集合拼出；其它 = 当前候选输出）。 */
  function landResult(landing: FloatingMenuLanding, text: string = result.output) {
    land(landing);
    clearPending();
    onLand(text, landing);
    reset();
  }

  function rejectResult() {
    if (resultActive) {
      land("discard");
    }
    clearPending();
    reset();
  }

  if (controlsRef) {
    controlsRef.current = {
      accept: () => {
        if (!canUseResult) {
          return false;
        }
        landResult(orderedLandings(scope, result.meta?.suggested_landing ?? null)[0]);
        return true;
      },
      escape: () => {
        if (running) {
          stop();
          return true;
        }
        handleClose();
        return true;
      },
    };
  }

  function runAction(action: FloatingMenuAction, options?: FloatingMenuOptionsDto) {
    setBackgroundOptions(options ?? NO_OPTIONS);
    void run(selectedText, action, {
      scope,
      noteRel: scope === "doc" && action === "summarize" ? notePath : null,
      context: contextEnabled ? context : null,
      options,
    });
  }

  /**
   * 转入后台（PRD-06-021）：整篇动作交给处理队列，面板即刻关闭，跑完发通知。
   * **只给整篇**——队列载荷只带笔记路径，选区那份范围离开面板就不再对得准。
   */
  function moveToBackground() {
    const action = result.action;
    if (!notePath || scope !== "doc" || !action) {
      return;
    }
    void commands.floatingMenuBackground(notePath, action, backgroundOptions);
    onClose();
  }

  function ask(instruction: string) {
    if (tableTask?.kind === "fill_column") {
      void run(selectedText, "table_fill_column", {
        scope,
        instruction,
        options: { ...NO_OPTIONS, table_column: tableTask.column },
      });
      return;
    }
    void run(selectedText, "ask", {
      scope,
      instruction,
      context: contextEnabled ? context : null,
    });
  }

  async function runGraphTags() {
    if (!onGraphTags || graphTags.status === "running") {
      return;
    }
    setGraphTags({ status: "running" });
    const outcome = await onGraphTags();
    setGraphTags(
      outcome.ok
        ? { status: "done", tags: outcome.tags }
        : { status: "failed", message: outcome.error },
    );
  }

  function activateSearchHit(hit: SearchHitDto) {
    onOpenPath?.(hit.path);
    onClose();
  }

  function handleKeyDown(event: React.KeyboardEvent<HTMLDivElement>) {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      if (running) {
        stop();
        return;
      }
      onClose();
      return;
    }
    if (event.key === "Enter" && (event.metaKey || event.ctrlKey) && canUseResult) {
      event.preventDefault();
      landResult(orderedLandings(scope, result.meta?.suggested_landing ?? null)[0]);
    }
  }

  // ── 动作清单（按作用域）────────────────────────────────────────────────────
  // 分组：编辑（改这一段）/ 生成 / 理解 / 本库。带子菜单的：翻译 ▸ 目标语言、总结 ▸ 风格、
  // 改语气 ▸ 档。语言清单与设置页的翻译目标语言同一张表（`TRANSLATE_TARGET_LANGS`）。
  // 预设：库里的可删；模板包带来的标出包名、随包卸载（ADR-169 ⑤），不给删除钮。
  const items: ComposerItem[] = presets.map((preset) => ({
    id: `preset-${preset.id}`,
    group: "presets" as const,
    label:
      preset.pack_name === null ? preset.name : t("floating.presets.fromPack", { name: preset.name, pack: preset.pack_name }),
    icon: <BookmarkIcon size={14} />,
    run: () => ask(preset.instruction),
    onRemove: preset.layer === "vault" ? () => deletePreset(preset.id) : undefined,
  }));
  // 预设一组的末尾：在线模板目录可用时给「获取更多模板」（AI 预设也来自模板包，PRD-18-004 / 006）。
  if (templateMarket !== null && templateMarket !== undefined) {
    items.push({
      id: "get-more-templates",
      group: "presets",
      label: t("workspace:apps.templates.getMore"),
      icon: <PlusIcon size={14} />,
      run: () => {
        onClose();
        goTo(APPS_TEMPLATES_ROUTE);
      },
    });
  }
  if (scope === "selection") {
    items.push(
      {
        id: "optimize",
        group: "edit",
        label: t("floating.action.optimize"),
        icon: <PencilIcon size={14} />,
        run: () => runAction("optimize"),
      },
      {
        id: "proofread",
        group: "edit",
        label: t("floating.action.proofread"),
        icon: <CheckIcon size={14} />,
        run: () => runAction("proofread"),
      },
      {
        id: "table_from_text",
        group: "edit",
        label: t("floating.action.table_from_text"),
        icon: <TableIcon size={14} />,
        run: () => runAction("table_from_text"),
      },
      {
        id: "tone",
        group: "edit",
        label: t("floating.action.tone"),
        icon: <SpeakIcon size={14} />,
        children: (["formal", "casual", "concise"] as const).map((tone) => ({
          id: `tone-${tone}`,
          group: "edit" as const,
          label: t(`floating.tone.${tone}`),
          icon: <SpeakIcon size={14} />,
          run: () =>
            runAction(
              tone === "formal" ? "tone_formal" : tone === "casual" ? "tone_casual" : "tone_concise",
            ),
        })),
      },
    );
  }
  // 脚本的改稿动作归「创作」应用：应用停用时这一组整个不出现（ADR-165 ④）。
  if (scope === "selection" && noteType === "script" && scriptActionsAvailable) {
    items.push({
      id: "script",
      group: "edit",
      label: t("floating.action.script"),
      icon: <FilmIcon size={14} />,
      children: (["tighten", "hook", "spoken", "dictation"] as const).map((kind) => ({
        id: `script-${kind}`,
        group: "edit" as const,
        label: t(`floating.script.${kind}`),
        icon: <FilmIcon size={14} />,
        run: () => runAction(`script_${kind}`),
      })),
    });
  }
  if (scope === "selection" && prompterActionsAvailable) {
    items.push({
      id: "prompter",
      group: "edit",
      label: t("floating.action.prompter"),
      icon: <ScrollTextIcon size={14} />,
      children: [
        {
          id: "prompter-spoken",
          group: "edit" as const,
          label: t("floating.prompter.spoken"),
          icon: <ScrollTextIcon size={14} />,
          run: () => runAction("prompter_spoken"),
        },
        ...PROMPTER_FIT_SECONDS.map((seconds) => ({
          id: `prompter-fit-${seconds}`,
          group: "edit" as const,
          label: t("floating.prompter.fit", { seconds }),
          icon: <ScrollTextIcon size={14} />,
          run: () => runAction("prompter_fit", { ...NO_OPTIONS, target_seconds: seconds }),
        })),
        {
          id: "prompter-cues",
          group: "edit" as const,
          label: t("floating.prompter.cues"),
          icon: <ScrollTextIcon size={14} />,
          run: () => runAction("prompter_cues"),
        },
      ],
    });
  }
  if (scope !== "doc") {
    items.push({
      id: "continue",
      group: "generate",
      label: t("floating.action.continue"),
      icon: <PlusIcon size={14} />,
      run: () => runAction("continue"),
    });
  }
  if (scope !== "cursor") {
    items.push({
      id: "summarize",
      group: "understand",
      label: t("floating.action.summarize"),
      icon: <SparklesIcon size={14} />,
      children: (["bullets", "paragraph", "one_line"] as const).map((style) => ({
        id: `summary-${style}`,
        group: "understand" as const,
        label: t(`floating.summary.${style}`),
        icon: <SparklesIcon size={14} />,
        run: () => runAction("summarize", { ...NO_OPTIONS, summary_style: style }),
      })),
    });
  }
  if (scope === "selection") {
    items.push(
      {
        id: "translate",
        group: "understand",
        label: t("floating.action.translate"),
        icon: <LanguagesIcon size={14} />,
        children: [
          {
            id: "translate-auto",
            group: "understand" as const,
            label: t("floating.translate.auto"),
            icon: <LanguagesIcon size={14} />,
            run: () => runAction("translate"),
          },
          ...TRANSLATE_TARGET_LANGS.map((code) => ({
            id: `translate-${code}`,
            group: "understand" as const,
            label: t(`common:lang.${code}`),
            icon: <LanguagesIcon size={14} />,
            run: () => runAction("translate", { ...NO_OPTIONS, target_lang: code }),
          })),
        ],
      },
      {
        id: "search",
        group: "vault",
        label: t("floating.action.search"),
        icon: <SearchIcon size={14} />,
        run: () => {
          void search(selectedText);
        },
      },
    );
  }
  if (scope === "doc" && onGraphTags) {
    items.push({
      id: "graph_tags",
      group: "vault",
      label: t("floating.action.graph_tags"),
      icon: <TagsIcon size={14} />,
      disabled: graphTags.status === "running",
      run: () => {
        void runGraphTags();
      },
    });
  }

  // ── 执行位置标记 ─────────────────────────────────────────────────────────
  const composerRouteLabel =
    onServer === null
      ? null
      : !onServer
        ? t("floating.route.local")
        : scope === "doc"
          ? t("floating.route.server_doc")
          : scope === "cursor"
            ? t("floating.route.server_cursor")
            : contextEnabled && hasContext
              ? t("floating.route.server_context")
              : t("floating.route.server_selection");
  const resultRouteLabel = result.meta
    ? [
        result.meta.on_server
          ? t("floating.route.server_sent", { count: result.meta.sent_chars })
          : t("floating.route.local"),
        result.meta.chunks > 0
          ? t(result.action === "table_fill_column" ? "floating.table.fill_rows" : "floating.preflight.chunked_done", { count: result.meta.chunks })
          : null,
      ]
        .filter(Boolean)
        .join(" · ")
    : null;
  const preflightNotice =
    preflight && preflight.strategy !== "direct"
      ? t(`floating.preflight.${preflight.strategy}`, {
          count: [...selectedText].length,
          limit: preflight.limit,
          chunks: preflight.chunks,
        })
      : null;

  // ── 摆放 ────────────────────────────────────────────────────────────────
  const measured = size ? { width: size.inline, height: size.block } : FALLBACK_SIZE;
  const auto = anchorBox
    ? placeFloatingMenu(
        { left: anchorBox.left, top: anchorBox.top, bottom: anchorBox.bottom },
        editorSize,
        measured,
      )
    : null;
  // 拖过之后由用户的位置说了算（整篇模式本来靠 CSS 停靠，一拖也转成显式坐标）。
  const placed = dragPos ?? auto;
  const maxBlock = placed
    ? Math.max(
        editorSize.height - placed.y - FLOATING_MENU_GAP,
        // 未拖动时还可以翻到锚点上方，故把那一侧的空间也算进上限。
        anchorBox && !dragPos ? anchorBox.top - FLOATING_MENU_GAP * 2 : 0,
        FLOATING_MENU_GAP * 4,
      )
    : null;

  return (
    <div
      ref={panelRef}
      className="luna-floating-menu"
      data-scope={scope}
      data-anchored={placed !== null}
      data-placement={dragPos ? undefined : auto?.placement}
      data-dragging={dragging || undefined}
      // 动态锚定坐标经 CSS 变量下发（与字幕面板不透明度同一先例），样式仍在 editor-core.css。
      style={
        placed && maxBlock !== null
          ? cssVariables({
              "--floating-menu-x": `${placed.x}px`,
              "--floating-menu-y": `${placed.y}px`,
              "--floating-menu-max-block": `${maxBlock}px`,
            })
          : undefined
      }
      role="dialog"
      aria-label={t("floating.title")}
      onKeyDown={handleKeyDown}
    >
      <header
        className="luna-floating-menu__header"
        title={t("floating.drag_hint")}
        onPointerDown={startDrag}
      >
        <span className="luna-floating-menu__title">
          <SparklesIcon size={14} className="luna-floating-menu__title-icon" />
          <span className="luna-floating-menu__scope">{t(`floating.scope.${scope}`)}</span>
        </span>
        <button
          type="button"
          className="luna-floating-menu__close"
          onClick={handleClose}
          title={t(running ? "floating.action.stop_close" : "floating.action.close")}
          aria-label={t(running ? "floating.action.stop_close" : "floating.action.close")}
        >
          <CloseIcon size={14} />
        </button>
      </header>
      {resultActive ? (
        <AiResultPanel
          scope={scope}
          result={result}
          originalText={selectedText}
          candidateIndex={candidateIndex}
          candidateCount={candidateCount}
          onSelectCandidate={selectCandidate}
          routeLabel={resultRouteLabel}
          acceptShortcut={acceptShortcut}
          onStop={stop}
          onRetry={retry}
          onReject={rejectResult}
          onCopy={(text) => {
            void commands.popupCopy(text);
          }}
          onSaveNote={(text) => {
            void commands.saveAiResultToNote(text);
          }}
          onLand={landResult}
          onFollowUp={(instruction) => {
            void followUp(instruction);
          }}
          onSavePreset={lastInstruction && result.action === "ask" ? savePreset : null}
          onExplain={explainChange}
          onBackground={scope === "doc" && notePath ? moveToBackground : null}
          fitTarget={fitTarget}
        />
      ) : searchActive ? null : (
        <AiComposer
          items={tableTask?.kind === "fill_column" ? [] : items}
          placeholder={
            tableTask?.kind === "fill_column"
              ? t("floating.table.fill_placeholder", { column: tableTask.header })
              : t("floating.ask_placeholder")
          }
          routeLabel={composerRouteLabel}
          contextToggle={
            scope === "selection" && hasContext
              ? { enabled: contextEnabled, onToggle: onToggleContext }
              : null
          }
          hint={
            toggleShortcut
              ? t("floating.hint", { toggle: toggleShortcut, accept: acceptShortcut })
              : t("floating.hint_no_toggle", { accept: acceptShortcut })
          }
          notice={status ?? preflightNotice}
          onAsk={ask}
        />
      )}
      {graphTagsActive ? (
        <section className="luna-floating-menu__result" aria-live="polite">
          {graphTags.status === "running" ? (
            <div className="luna-floating-menu__status">
              <span className="luna-floating-menu__spinner" aria-hidden="true" />
              <span>{t("graphTags.generating")}</span>
            </div>
          ) : null}
          {graphTags.status === "failed" ? (
            <>
              <p className="luna-floating-menu__status" role="status">
                {t("floating.status.failed")}: {graphTags.message}
              </p>
              <div className="luna-floating-menu__landing">
                <button
                  type="button"
                  className="luna-btn luna-btn--secondary"
                  onClick={() => {
                    void runGraphTags();
                  }}
                >
                  {t("floating.action.retry")}
                </button>
                <button
                  type="button"
                  className="luna-btn luna-btn--secondary"
                  onClick={() => setGraphTags({ status: "idle" })}
                >
                  {t("floating.action.reject")}
                </button>
              </div>
            </>
          ) : null}
          {graphTags.status === "done" ? (
            <>
              <p className="luna-floating-menu__notice" role="status">
                {t("graphTags.panelDone", { count: graphTags.tags.length })}
              </p>
              <div className="luna-floating-menu__tags">
                {graphTags.tags.map((tag) => (
                  <span key={tag} className="luna-floating-menu__tag">
                    #{tag}
                  </span>
                ))}
              </div>
              <div className="luna-floating-menu__landing">
                {onViewGraph ? (
                  <button
                    type="button"
                    className="luna-btn luna-btn--secondary"
                    onClick={() => {
                      onViewGraph();
                      onClose();
                    }}
                  >
                    <GraphIcon size={14} />
                    {t("graphTags.viewGraph")}
                  </button>
                ) : null}
                <button
                  type="button"
                  className="luna-btn luna-btn--secondary"
                  onClick={() => setGraphTags({ status: "idle" })}
                >
                  {t("graphTags.ok")}
                </button>
              </div>
            </>
          ) : null}
        </section>
      ) : null}
      {searchActive ? (
        <section className="luna-floating-menu__search">
          <p>
            {searchState.status === "loading"
              ? t("floating.search.loading")
              : searchState.status === "error"
                ? t("floating.search.error")
                : t("floating.search.count", { count: searchState.hits.length })}
          </p>
          <FilteredList
            className="luna-command-list"
            items={searchState.hits}
            query=""
            getKey={(hit) => hit.path}
            getText={(hit) => `${hit.title} ${hit.path} ${hit.snippet}`}
            empty={<p className="luna-empty">{t("floating.search.empty")}</p>}
            onActivate={activateSearchHit}
            onCancel={onClose}
            renderRow={({ item }) => (
              <>
                <strong>{item.title || item.path}</strong>
                <small>{item.path}</small>
                <small>{item.snippet}</small>
              </>
            )}
          />
        </section>
      ) : null}
    </div>
  );
}
