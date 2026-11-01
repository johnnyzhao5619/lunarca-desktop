/**
 * AI 对话视图（PRD-03）：主窗口「对话」标签页与快捷面板（`quick-chat` 非激活 NSPanel）
 * 共用。会话事实源在 Rust（scope 键控，PRD-03-012）；本组件只发命令、订阅事件、渲染
 * （ADR-003）。对标 ChatGPT/Claude 的成熟交互：全量 Markdown 渲染、智能贴底滚动、
 * 空态角色预设、消息悬浮操作条、结构化错误卡、停止/重新生成、关闭未存提示。
 */

import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type MouseEvent as ReactMouseEvent,
} from "react";
import { useTranslation } from "react-i18next";
import { IconButton } from "../../components/ui/IconButton";
import { HudSurface } from "../../components/ui/HudSurface";
import { HudHeader } from "../../components/ui/HudHeader";
import { HudReceipt } from "../../components/ui/HudReceipt";
import { useWindowDrag } from "../../hooks/use-window-drag";
import { usePanelTextEditing } from "../../hooks/use-panel-text-editing";
import {
  commands,
  events,
  type ChatPresetDto,
  type ComputeStatusDto,
  type MentionCandidateDto,
  type MentionScopeDto,
  type ModelEntryDto,
} from "../../ipc/bindings";
import {
  ArrowClockwiseIcon,
  CheckIcon,
  ChevronDownIcon,
  ClockIcon,
  CloseIcon,
  CopyIcon,
  ExternalWindowIcon,
  GhostIcon,
  NotebookIcon,
  PlusIcon,
  QuoteIcon,
  SendIcon,
  SparklesIcon,
  StopCircleIcon,
  BookOpenIcon,
} from "../../components/ui/Icon";
import { ChatMarkdown } from "./ChatMarkdown";
import { ChatSources, citationTextRenderer, useSourceOpener } from "./ChatSources";
import { DistillBadge } from "../distill/DistillBadge";
import { ChatHistoryList } from "./ChatHistoryList";
import { ListLoadMore } from "../workspace/ListLoadMore";
import { useNoteSelection } from "../workspace/use-note-selection";
import { useNoteMenu } from "../workspace/note-menu";
import { useDeleteConsequence } from "../../hooks/use-delete-mode";
import { HudActionMenu } from "../../components/ui/HudActionMenu";
import { FailureCard } from "../../components/ui/FailureCard";
import { activeMention, applyMention } from "./chat-mentions";
import type { ChatController } from "./use-chat-controller";
import { useChatSession, type ChatUiMessage, type ReaskOverrides } from "./use-chat-session";
import { useStickToBottom } from "./use-stick-to-bottom";
import { formatShortcut } from "../../lib/shortcut-display";
import { noteStem } from "../../lib/note-path";
import { EmptyState } from "../../components/ui/EmptyState";
import { useContextMenu } from "../../components/ContextMenuHost";
import { buildMenu, separator, type ContextMenuItem } from "../../lib/context-menu";
import { goTo, openNote } from "../../lib/navigate";
import { isImeKeyEvent } from "../../lib/ime-key-event";

interface ChatViewProps {
  /** 共享会话控制器（主区对话与主侧栏历史共用同一实例，PRD-03-013）。 */
  controller: ChatController;
  panel?: boolean;
}

export function resolveChatRouteLabel(
  routeKind: ComputeStatusDto["chat_route_kind"] | null,
  activeModelName: string | undefined,
  resolved: boolean,
  translate: (key: string) => string,
): string {
  if (!resolved) return translate("route.loading");
  if (routeKind === "managed_service") return translate("route.managed_service");
  if (routeKind === null) return translate("route.unavailable");
  return activeModelName ?? translate("route.local_unconfigured");
}

declare global {
  interface Window {
    __lunaFocusChatInput?: () => void;
  }
}

/** 选区上下文 + 用户问题 → 单条 user 消息（上下文以引用块前置，历史里可读可溯）。 */
export function composeWithContext(input: string, context: string | null): string {
  const question = input.trim();
  if (!context || !context.trim()) {
    return question;
  }
  const quoted = context
    .trimEnd()
    .split("\n")
    .map((line) => `> ${line}`)
    .join("\n");
  return question ? `${quoted}\n\n${question}` : quoted;
}

const NOTICE_DISMISS_MS = 2400;
const PREFILL_PREVIEW_CHARS = 96;

export function ChatView({ controller, panel = false }: ChatViewProps) {
  const { t } = useTranslation("chat");
  const { t: tCommonAction } = useTranslation("common");
  // 行右键与主侧栏同一个构建器；「删了之后会怎样」同一处取词。
  const historyMenu = useNoteMenu();
  const historyContextMenu = useContextMenu();
  const deleteConsequence = useDeleteConsequence();
  /** `@` 提及的文案与建议条/主题页同住 workspace 命名空间（同一件事的三个表面）。 */
  const { t: tw } = useTranslation("workspace");
  const { history } = controller;
  // 整份订阅只在这一层：流式回答一个 token 重渲染的是对话视图，不是持有控制器的宿主。
  const session = useChatSession(controller.store);

  const [input, setInput] = useState("");
  /** `@` 定范围（PRD-16-009）：本条消息的范围芯片 + 候选面板三件套。 */
  const [mentions, setMentions] = useState<MentionScopeDto[]>([]);
  const [mentionQuery, setMentionQuery] = useState<string | null>(null);
  const [mentionOptions, setMentionOptions] = useState<MentionCandidateDto[]>([]);
  const [mentionIndex, setMentionIndex] = useState(0);
  const [models, setModels] = useState<ModelEntryDto[]>([]);
  const [computeStatus, setComputeStatus] = useState<ComputeStatusDto | null>(null);
  const [routeResolved, setRouteResolved] = useState(false);
  const [presets, setPresets] = useState<ChatPresetDto[]>([]);
  const [prefill, setPrefill] = useState<string | null>(null);
  /** 短暂提示（复制 / 保存 / 打开失败…）：面板形态走回执线（ADR-137 V5），主窗口在页头。 */
  const [notice, setNotice] = useState<{ text: string; kind: "success" | "error" } | null>(null);
  const [shortcutHint, setShortcutHint] = useState("");
  const [historyOpen, setHistoryOpen] = useState(false);

  const inputRef = useRef<HTMLTextAreaElement | null>(null);
  const composerEditing = usePanelTextEditing();
  const messagesRef = useRef<HTMLDivElement | null>(null);
  const noticeTimerRef = useRef<number | null>(null);

  const { onPointerDown: handleDrag } = useWindowDrag();

  const lastMessage = session.messages[session.messages.length - 1];
  const contentKey = `${session.messages.length}:${lastMessage?.content.length ?? 0}:${lastMessage?.status ?? ""}`;
  const { pinned, scrollToBottom } = useStickToBottom(messagesRef, contentKey);

  const showNotice = useCallback((text: string, kind: "success" | "error" = "success") => {
    setNotice({ text, kind });
    if (noticeTimerRef.current !== null) {
      window.clearTimeout(noticeTimerRef.current);
    }
    noticeTimerRef.current = window.setTimeout(() => setNotice(null), NOTICE_DISMISS_MS);
  }, []);

  // 执行路径与本机目录并行读取：顶栏先回答「在哪跑/谁选模型」，只有本机档才显示模型名。
  // 不能再从本机目录反推云端状态——服务端托管时本地可以没有任何模型，但对话完全可用。
  useEffect(() => {
    const loadRoute = async () => {
      const [modelsResult, statusResult] = await Promise.all([
        commands.listModels(),
        commands.computeStatus(),
      ]);
      if (modelsResult.status === "ok") {
        const llm = modelsResult.data.filter(
          (m) =>
            m.effective_capabilities.includes("chat") &&
            (m.status === "installed" || m.status === "bundled"),
        );
        setModels(llm);
      }
      setComputeStatus(statusResult.status === "ok" ? statusResult.data : null);
      setRouteResolved(true);
    };
    void loadRoute();
    void commands.listChatPresets().then(setPresets);
    const unlisten = events.settingsChanged.listen(() => void loadRoute());
    return () => {
      void unlisten.then((dispose) => {
        dispose();
      });
    };
  }, []);

  // 面板空态展示真实全局快捷键（单一来源：settings registry + shortcut-display）。
  useEffect(() => {
    if (!panel) {
      return;
    }
    void commands.listSettings().then((result) => {
      if (result.status !== "ok") {
        return;
      }
      const spec = result.data.find((s) => s.key === "shortcuts.quick_chat");
      if (spec && spec.current_value.kind === "str") {
        setShortcutHint(formatShortcut(spec.current_value.value));
      }
    });
  }, [panel]);

  // 选区带入（PRD-03-008）：就绪事件 → 单次拉取正文；挂载时兜底消费一次待带入内容。
  useEffect(() => {
    if (!panel) {
      return;
    }
    async function take() {
      const result = await commands.chatPrefillTake();
      if (result.status === "ok" && result.data) {
        setPrefill(result.data);
      }
    }
    void take();
    const unlisten = events.chatPrefill.listen(() => {
      void take();
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [panel]);

  // 面板焦点桥（show_quick_chat_panel 经 eval 调用）。
  useEffect(() => {
    window.__lunaFocusChatInput = () => {
      inputRef.current?.focus();
    };
    return () => {
      delete window.__lunaFocusChatInput;
    };
  }, []);

  // 关闭面板即隐藏（会话保留在内存/已按持久性归档，PRD-03-012 无"意外丢失"，无需拦截提示）。
  const closePanel = useCallback(() => {
    void commands.quickChatClose();
  }, []);


  const sendCurrent = useCallback(async () => {
    const message = composeWithContext(input, prefill);
    if (!message || session.isRunning) {
      return;
    }
    setInput("");
    setPrefill(null);
    // `@` 框定的范围随**这一条消息**走（PRD-16-009）：发出去即清空芯片行，
    // 下一个问题重新按默认口径检索——「这一次看哪儿」不是会话级偏好。
    const scoped = mentions;
    setMentions([]);
    setMentionQuery(null);
    await session.sendWithScope(message, scoped);
  }, [input, mentions, prefill, session]);

  const copyText = useCallback(
    async (text: string) => {
      const result = await commands.popupCopy(text);
      showNotice(
        result.status === "ok" ? t("notice.copied") : t("notice.copy_failed"),
        result.status === "ok" ? "success" : "error",
      );
    },
    [showNotice, t],
  );

  /**
   * 引用一条消息到输入框（Markdown 引用块 + 空行）：把"针对这一段追问"变成一个动作，
   * 而不是让用户手动选中、复制、粘贴、再补上 `>`。
   */
  const quoteMessage = useCallback((text: string) => {
    setInput((current) => {
      const quoted = text
        .split("\n")
        .map((line) => `> ${line}`)
        .join("\n");
      return current ? `${current}\n\n${quoted}\n\n` : `${quoted}\n\n`;
    });
    inputRef.current?.focus();
  }, []);

  const saveConversation = useCallback(async () => {
    const path = await session.saveToNote();
    showNotice(path ? t("notice.saved") : t("notice.save_failed"), path ? "success" : "error");
    return path;
  }, [session, showNotice, t]);

  const continueInWorkspace = useCallback(async () => {
    if (session.isRunning) {
      return;
    }
    let path = session.notePath;
    if (!path && session.messages.length > 0) {
      path = await session.saveToNote();
      if (!path) {
        showNotice(t("notice.continue_failed"), "error");
        return;
      }
    }
    const result = await commands.openWorkspaceRoute(`chats://${path ?? ""}`);
    if (result.status !== "ok") {
      showNotice(t("notice.continue_failed"), "error");
      return;
    }
    await commands.quickChatClose();
  }, [session, showNotice, t]);

  // 切换「临时对话」（PRD-03-012）：开新会话并覆盖持久性。旧持久会话已逐轮进历史，切换不丢内容。
  const toggleTemporary = useCallback(async () => {
    await session.reset(session.presetId, !session.temporary);
    setHistoryOpen(false);
    inputRef.current?.focus();
  }, [session]);

  /** 按路径打开一篇笔记（导航语义单一：全应用都走 `luna:navigate`）。 */
  const openNoteByPath = useCallback((path: string) => {
    openNote(path);
  }, []);

  // 「在笔记库中打开」当前会话链接的归档笔记（仅主窗口，单一导航语义）。
  const openLinkedNote = useCallback(() => {
    if (session.notePath) {
      openNote(session.notePath);
    }
  }, [session.notePath]);

  const saveMessage = useCallback(
    async (content: string) => {
      const result = await commands.saveAiResultToNote(content);
      showNotice(
        result.status === "ok" ? t("notice.saved") : t("notice.save_failed"),
        result.status === "ok" ? "success" : "error",
      );
    },
    [showNotice, t],
  );

  /** 洞见回流（PRD-16-010）：整条存为知识笔记，「来源」区由 Rust 按引用过的笔记拼。 */
  const saveAsKnowledge = useCallback(
    async (messageIndex: number) => {
      const path = await session.saveMessageToNote(messageIndex);
      showNotice(
        path ? t("notice.saved_knowledge", { title: noteStem(path) }) : t("notice.save_failed"),
        path ? "success" : "error",
      );
    },
    [session, showNotice, t],
  );

  const startPresetChat = useCallback(
    async (nextPresetId: string | null) => {
      await controller.startPresetChat(nextPresetId);
      setHistoryOpen(false);
      inputRef.current?.focus();
    },
    [controller],
  );

  // 历史会话（PRD-03-013）：载入 / 删除（删除当前链接的会话则同时开新会话，逻辑在控制器）。
  const openHistoryEntry = useCallback(
    async (path: string) => {
      const ok = await controller.openHistoryEntry(path);
      if (!ok) {
        showNotice(t("history.open_failed"), "error");
      }
      setHistoryOpen(false);
      inputRef.current?.focus();
    },
    [controller, showNotice, t],
  );

  const deleteHistoryEntries = useCallback(
    async (paths: readonly string[]) => {
      const removal = await controller.deleteHistoryEntries(paths);
      if (removal.failed) {
        showNotice(t("history.delete_failed"), "error");
        return;
      }
      if (removal.deleted.length > 0) {
        showNotice(t("history.deleted"));
      }
    },
    [controller, showNotice, t],
  );

  /**
   * 弹层里的历史会话同样可多选（PRD-06-059）：它与主侧栏是同一份渲染，"这里能、那里不能"
   * 正是同一件事长出两种做法的开始。
   */
  const [historyRows, setHistoryRows] = useState<readonly string[]>([]);
  const historySelection = useNoteSelection({
    current: session.notePath,
    visible: historyRows,
    // 函数声明而非箭头常量：它要读的 `historySelection` 正是自己下面那一行的结果，
    // Delete 键按下时才求值。
    onDelete: () => deleteHistorySelection(),
  });

  function deleteHistorySelection() {
    void deleteHistoryEntries(historySelection.ordered);
  }

  const presetLabel = useCallback(
    (preset: ChatPresetDto) => t(`preset.${preset.id}`, { defaultValue: preset.name }),
    [t],
  );

  const activePreset = useMemo(
    () => presets.find((preset) => preset.id === session.presetId) ?? null,
    [presets, session.presetId],
  );

  /** 输入变化时重判「光标是不是正在写一个 `@` 提及」（判定是纯函数，见 chat-mentions）。 */
  const syncMention = useCallback((value: string, caret: number) => {
    const active = activeMention(value, caret);
    setMentionQuery(active ? active.query : null);
    setMentionIndex(0);
  }, []);

  // 候选恒由 Rust 给（「什么算一个可提及的范围」与「某篇算不算在范围内」是同一个问题的
  // 两半，前端自己算必然与 `luna_vault::in_scope` 漂移）。
  useEffect(() => {
    if (mentionQuery === null) {
      setMentionOptions([]);
      return;
    }
    let disposed = false;
    void commands.mentionCandidates(mentionQuery).then((result) => {
      if (!disposed) {
        setMentionOptions(result.status === "ok" ? result.data : []);
      }
    });
    return () => {
      disposed = true;
    };
  }, [mentionQuery]);

  const pickMention = useCallback(
    (option: MentionCandidateDto) => {
      const textarea = inputRef.current;
      const caret = textarea?.selectionStart ?? input.length;
      const active = activeMention(input, caret);
      if (active) {
        const next = applyMention(input, active);
        setInput(next.text);
        window.requestAnimationFrame(() => {
          textarea?.setSelectionRange(next.caret, next.caret);
          textarea?.focus();
        });
      }
      setMentions((current) =>
        current.some((m) => m.kind === option.kind && m.value === option.value)
          ? current
          : [...current, { kind: option.kind, value: option.value }],
      );
      setMentionQuery(null);
    },
    [input],
  );

  const handleKeyDown = (event: React.KeyboardEvent<HTMLTextAreaElement>) => {
    // 只判 `isComposing` 在 WKWebView 里拦不住上屏那一下（PRD-07-040 ①）。
    if (isImeKeyEvent(event)) {
      return;
    }
    // `@` 候选面板开着时，方向键与回车归它——否则用户刚打开候选、想按回车选中，
    // 消息就被发出去了（这类"抢键"是补全交互最常见的失败方式）。
    if (mentionOptions.length > 0 && mentionQuery !== null) {
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        setMentionIndex((current) => {
          const next = current + (event.key === "ArrowDown" ? 1 : -1);
          return (next + mentionOptions.length) % mentionOptions.length;
        });
        return;
      }
      if (event.key === "Enter" || event.key === "Tab") {
        event.preventDefault();
        pickMention(mentionOptions[mentionIndex] ?? mentionOptions[0]!);
        return;
      }
      if (event.key === "Escape") {
        // 分层让路：Esc 只收起 @ 候选，不许冒泡到外壳把整个面板关掉。
        event.preventDefault();
        event.stopPropagation();
        setMentionQuery(null);
        return;
      }
    }
    if (event.key === "Enter" && !event.shiftKey && !event.metaKey && !event.ctrlKey) {
      event.preventDefault();
      void sendCurrent();
    }
  };

  const isEmpty = session.messages.length === 0;
  const canSend = composeWithContext(input, prefill).length > 0;
  const activeModelName = models.find((model) =>
    model.assigned_slots.includes("default_llm"),
  )?.name;
  const routeLabel = resolveChatRouteLabel(
    computeStatus?.chat_route_kind ?? null,
    activeModelName,
    routeResolved,
    t,
  );

  // 顶栏模型徽标（两种形态共用）：只读展示本轮有效 LLM，点击深链到模型分配
  // （PRD-03-003）——全局默认的唯一写入口在「AI 引擎 / 模型」，顶栏绝不再提供
  // 一个写全局默认的下拉。
  const modelBadge = (
    <button
      type="button"
      className="luna-chat__model-badge"
      title={t("model_selector_open_assignment")}
      aria-label={t("model_selector_label")}
      onClick={() => {
        goTo("settings://ai_services");
      }}
    >
      {routeLabel}
    </button>
  );

  // 面板窗口栏（HudHeader 五槽，ADR-111 ②）：身份 Sparkles · 配置胶囊（临时/角色，
  // 唯一皮 `.luna-hud-context-badge`——主窗口页头保留自己的徽标形态，那是页面不是 HUD）
  // · 动作区（历史/更多/关闭）；历史弹层锚在窗口栏上（children 槽）。
  const panelHeader = (
    <HudHeader
      className="luna-chat-panel__header"
      onPointerDown={handleDrag}
      data-draggable="true"
      icon={<SparklesIcon size={15} />}
      title={t("title")}
      context={
        session.temporary ? (
          <>
            <GhostIcon size={12} />
            {t("temporary.badge")}
          </>
        ) : activePreset ? (
          presetLabel(activePreset)
        ) : undefined
      }
      // 模型是只读信息（点开去设置）：放在状态行的余宽里、纯文字，不再是同排最重的那一枚
      // 徽标（ADR-137 H7）。
      lane={modelBadge}
      actions={
        <>
          <IconButton
            density="hud"
            label={t("history.title")}
            icon={<ClockIcon size={15} />}
            aria-pressed={historyOpen}
            onClick={() => setHistoryOpen((open) => !open)}
          />
          <HudActionMenu
            label={t("action.more")}
            side="bottom"
            shortcut
            items={[
              {
                id: "continue",
                label: t("action.continue_workspace"),
                icon: <ExternalWindowIcon size={14} />,
                disabled: session.isRunning || isEmpty,
                onSelect: () => void continueInWorkspace(),
              },
              {
                id: "temporary",
                label: t("temporary.toggle"),
                icon: <GhostIcon size={14} />,
                checked: session.temporary,
                disabled: session.isRunning,
                onSelect: () => void toggleTemporary(),
              },
              {
                id: "new",
                label: t("action.new"),
                icon: <PlusIcon size={14} />,
                disabled:
                  session.isRunning || (isEmpty && !session.presetId && !session.temporary),
                onSelect: () => void startPresetChat(session.presetId),
              },
              ...(!session.notePath
                ? [
                    {
                      id: "save",
                      label: t("action.save_note"),
                      icon: <NotebookIcon size={14} />,
                      disabled: isEmpty,
                      onSelect: () => void saveConversation(),
                    },
                  ]
                : []),
            ]}
          />
          <IconButton
            density="hud"
            label={t("action.close")}
            icon={<CloseIcon size={15} />}
            onClick={closePanel}
          />
        </>
      }
    >
      {historyOpen ? (
        <div className="luna-chat-history-popover">
          <ChatHistoryList
            entries={history.entries}
            loadState={history.page}
            footer={
              (history.page.loading || history.page.failed) && (
                <ListLoadMore
                  loading={history.page.loading}
                  failed={history.page.failed}
                  exhausted={history.page.exhausted}
                  onLoadMore={history.page.loadMore}
                  onRetry={() => void history.refresh()}
                />
              )
            }
            total={history.total}
            activePath={session.notePath}
            canOpenInNotes={false}
            onNewChat={() => void startPresetChat(session.presetId)}
            onOpen={(path) => void openHistoryEntry(path)}
            onDelete={(paths) => void deleteHistoryEntries(paths)}
            selection={historySelection}
            onVisibleRows={setHistoryRows}
            // 与主侧栏**同一个**菜单构建器，只是这扇窗给不了的那几项不传（独立窗口打开、
            // 移动到文件夹都要主窗口在场）——不是另写一份精简菜单。
            onItemContextMenu={(event, entry, targets) =>
              historyContextMenu.open(
                event,
                historyMenu({
                  path: entry.path,
                  paths: targets,
                  distillable: history.distillable,
                  mergeable:
                    history.mergeable &&
                    targets.length >= history.mergeBounds.min &&
                    targets.length <= history.mergeBounds.max,
                  rename: { onRenamed: () => void history.refresh() },
                  remove: {
                    confirm: {
                      title: t("history.delete_title"),
                      description: deleteConsequence,
                      confirmLabel: tCommonAction("action.delete"),
                    },
                    onDeleted: () => void history.refresh(),
                  },
                  onStatus: showNotice,
                  onChanged: () => void history.refresh(),
                }),
              )
            }
          />
        </div>
      ) : null}
    </HudHeader>
  );

  // 主窗口页头：h1 + 主区徽标形态（它是工作区的一页，不受 HUD 五槽契约辖）。
  const mainHeader = (
    <header className="luna-main-header luna-chat__header">
      <div className="luna-main-header__title">
        <h1>{t("title")}</h1>
        {session.temporary ? (
          <span className="luna-chat__temp-badge">
            <GhostIcon size={12} />
            {t("temporary.badge")}
          </span>
        ) : activePreset ? (
          <span className="luna-chat__preset-badge">{presetLabel(activePreset)}</span>
        ) : null}
      </div>
      <div className="luna-main-header__actions">
        {/* 知识笔记徽标（PRD-03-014）：把这次对话的结论沉淀成一篇笔记。
            只在主窗口且会话已落盘时出现——临时对话没有源笔记，快捷面板空间也不够。 */}
        <DistillBadge notePath={session.notePath} onOpenNote={openNoteByPath} />
        {notice ? (
          <span className="luna-chat__notice" data-kind={notice.kind}>
            {notice.text}
          </span>
        ) : null}
        {modelBadge}
        <IconButton
          label={t("temporary.toggle")}
          icon={<GhostIcon size={16} />}
          aria-pressed={session.temporary}
          disabled={session.isRunning}
          onClick={() => void toggleTemporary()}
        />
        <IconButton
          label={t("action.new")}
          icon={<PlusIcon size={16} />}
          disabled={session.isRunning || (isEmpty && !session.presetId && !session.temporary)}
          onClick={() => void startPresetChat(session.presetId)}
        />
        {session.notePath ? (
          <IconButton
            label={t("action.open_in_notes")}
            icon={<NotebookIcon size={16} />}
            onClick={openLinkedNote}
          />
        ) : (
          <IconButton
            label={t("action.save_note")}
            icon={<NotebookIcon size={16} />}
            disabled={isEmpty}
            onClick={() => void saveConversation()}
          />
        )}
      </div>
    </header>
  );

  const chatSurface = (
    <section className="luna-chat" data-panel={panel}>
      {panel ? panelHeader : mainHeader}

      {panel && historyOpen ? (
        <div className="luna-chat-history-backdrop" onPointerDown={() => setHistoryOpen(false)} />
      ) : null}

      <div className="luna-chat__body">
        <div className="luna-chat__messages" ref={messagesRef} aria-live="polite">
          {isEmpty ? (
            <EmptyState
              icon={<SparklesIcon size={panel ? 18 : 24} />}
              title={t("empty.title")}
              description={
                session.temporary
                  ? t("empty.hint_temporary")
                  : panel && shortcutHint
                    ? t("empty.hint_panel", { shortcut: shortcutHint })
                    : t("empty.hint")
              }
            >
              {presets.length > 0 ? (
                <div
                  className="luna-chat__preset-chips"
                  role="group"
                  aria-label={t("empty.presets_label")}
                >
                  {presets.map((preset) => (
                    <button
                      key={preset.id}
                      type="button"
                      className="luna-chat__preset-chip"
                      data-active={session.presetId === preset.id}
                      onClick={() =>
                        void startPresetChat(session.presetId === preset.id ? null : preset.id)
                      }
                    >
                      {presetLabel(preset)}
                    </button>
                  ))}
                </div>
              ) : null}
            </EmptyState>
          ) : (
            session.messages.map((message, index) => (
              <ChatMessageRow
                key={message.id}
                message={message}
                isLast={index === session.messages.length - 1}
                isRunning={session.isRunning}
                onCopy={copyText}
                onSave={saveMessage}
                onSaveAsKnowledge={() => saveAsKnowledge(index)}
                onReask={(overrides) => void session.reask(overrides)}
                onQuote={quoteMessage}
                onRegenerate={() => void session.regenerate()}
              />
            ))
          )}
        </div>
        {!pinned && !isEmpty ? (
          <IconButton
            className="luna-chat__jump-latest"
            label={t("action.jump_latest")}
            icon={<ChevronDownIcon size={16} />}
            onClick={() => scrollToBottom("smooth")}
          />
        ) : null}
      </div>

      {panel ? (
        <HudReceipt
          receipt={notice ? { kind: notice.kind, message: notice.text } : null}
          onExpire={() => setNotice(null)}
        />
      ) : null}
      <form
        className="luna-chat__composer"
        onSubmit={(event) => {
          event.preventDefault();
          void sendCurrent();
        }}
      >
        {prefill ? (
          <div className="luna-chat__context-chip" title={prefill}>
            <span className="luna-chat__context-chip-label">{t("context.label")}</span>
            <span className="luna-chat__context-chip-text">
              {prefill.length > PREFILL_PREVIEW_CHARS
                ? `${prefill.slice(0, PREFILL_PREVIEW_CHARS)}…`
                : prefill}
            </span>
            <IconButton
              density="sm"
              label={t("context.remove")}
              icon={<CloseIcon size={12} />}
              onClick={() => setPrefill(null)}
            />
          </div>
        ) : null}
        {/* `@` 候选（PRD-16-009）：范围控制的第三层，落在输入框正上方——它属于
            "这一条消息"，与会话级的知识库芯片、事后纠正的透明行各在其位。 */}
        {mentionQuery !== null && mentionOptions.length > 0 ? (
          <ul
            className="luna-chat__mention-list"
            role="listbox"
            aria-label={tw("mention.listLabel")}
          >
            {mentionOptions.map((option, index) => (
              <li key={`${option.kind}:${option.value}`}>
                <button
                  type="button"
                  role="option"
                  aria-selected={index === mentionIndex}
                  data-active={index === mentionIndex || undefined}
                  onMouseEnter={() => setMentionIndex(index)}
                  onClick={() => pickMention(option)}
                >
                  <span className="luna-chat__mention-kind">
                    {tw(`mention.kind.${option.kind}`)}
                  </span>
                  <span className="luna-chat__mention-label">{option.label}</span>
                </button>
              </li>
            ))}
          </ul>
        ) : null}
        {mentions.length > 0 ? (
          <div className="luna-chat__mention-chips" aria-label={tw("mention.chipsLabel")}>
            {mentions.map((mention) => (
              <span key={`${mention.kind}:${mention.value}`} className="luna-chat__mention-chip">
                <span className="luna-chat__mention-kind">
                  {tw(`mention.kind.${mention.kind}`)}
                </span>
                {mention.value}
                <button
                  type="button"
                  title={tw("mention.remove")}
                  aria-label={tw("mention.remove")}
                  onClick={() =>
                    setMentions((current) =>
                      current.filter(
                        (m) => !(m.kind === mention.kind && m.value === mention.value),
                      ),
                    )
                  }
                >
                  <CloseIcon size={11} />
                </button>
              </span>
            ))}
          </div>
        ) : null}
        {/* 输入框随内容长（镜像格子，同速记与翻译框，ADR-137 H7）：此前每敲一个字在 JS 里量
            一次 scrollHeight 再写回内联高度——那是内联样式（TDD-15 §3.2 禁），也是每键一次布局读。 */}
        <div className="luna-chat__composer-grow" data-value={input}>
          <textarea
            ref={inputRef}
            value={input}
            placeholder={t("input.placeholder")}
            onFocus={composerEditing.onFocus}
            onBlur={composerEditing.onBlur}
            onChange={(event) => {
              setInput(event.target.value);
              syncMention(event.target.value, event.target.selectionStart ?? 0);
            }}
            onKeyDown={handleKeyDown}
            rows={1}
          />
        </div>
        <div className="luna-chat__composer-footer">
          <button
            type="button"
            className="luna-chat__grounding-chip"
            aria-pressed={session.grounding}
            title={session.grounding ? t("grounding.on_hint") : t("grounding.off_hint")}
            onClick={() => void session.setGrounding(!session.grounding)}
          >
            <BookOpenIcon size={12} />
            <span>{t("grounding.label")}</span>
          </button>
          {/* 单条脚注（ADR-111 ⑤⑷）：`@` 的用法在 `@` 被敲出来时才说——两条并列的
              操作说明在面板宽度下互相挤压，而且在互相竞争注意力。 */}
          <span className="luna-chat__composer-hint">
            {mentionQuery !== null ? tw("mention.hint") : t("input.hint")}
          </span>
          <div className="luna-chat__composer-actions">
            {session.isRunning ? (
              <IconButton
                tone="danger"
                label={t("action.stop")}
                icon={<StopCircleIcon size={18} />}
                onClick={() => void session.stop()}
              />
            ) : (
              <IconButton
                tone="primary"
                label={t("action.send")}
                icon={<SendIcon size={16} />}
                type="submit"
                disabled={!canSend}
              />
            )}
          </div>
        </div>
      </form>
    </section>
  );

  // 主区只渲染对话本体；历史会话列表由工作区「主侧栏」承载（PRD-07-057），面板变体则用
  // 头部弹层（上方 `historyOpen`）。二者共用同一 `controller`（单一会话事实源）。
  // 面板变体挂 HUD 外壳（ADR-111 ②）：浮层皮与 Esc 归外壳；`dragRoot={false}`——
  // 对话只由头部拖动，正文要保持可选中。Esc 分层让路：历史弹层开着先关它。
  if (panel) {
    return (
      <HudSurface
        aria-label={t("title")}
        dragRoot={false}
        onDismiss={() => {
          if (historyOpen) {
            setHistoryOpen(false);
            return;
          }
          closePanel();
        }}
      >
        {chatSurface}
      </HudSurface>
    );
  }
  return chatSurface;
}

interface ChatMessageRowProps {
  message: ChatUiMessage;
  isLast: boolean;
  isRunning: boolean;
  onCopy: (text: string) => Promise<void>;
  onSave: (text: string) => Promise<void>;
  /** 洞见回流（PRD-16-010）：整条存为知识笔记，来源区由 Rust 拼。 */
  onSaveAsKnowledge: () => Promise<void>;
  /** 换一批检索依据重答（PRD-16-006）。 */
  onReask: (overrides: ReaskOverrides) => void;
  /** 引用这条消息到输入框（右键菜单，PRD-03）。 */
  onQuote: (text: string) => void;
  onRegenerate: () => void;
}

function ChatMessageRow({
  message,
  isLast,
  isRunning,
  onCopy,
  onSave,
  onSaveAsKnowledge,
  onReask,
  onQuote,
  onRegenerate,
}: ChatMessageRowProps) {
  const { t } = useTranslation("chat");
  const { t: tCommon } = useTranslation("common");
  const contextMenu = useContextMenu();
  const [copied, setCopied] = useState(false);
  // 失败呈现走唯一注册表（common:failure.* + FailureCard，ADR-111 ④ 批 E2）；
  // 未知码由注册表的 generic 兜底，这里不再改写码。
  const failure =
    message.status === "failed"
      ? (message.failure ?? { code: "UNKNOWN", retriable: true, retry_after_secs: null })
      : null;
  const serverRetryAfter = failure?.retry_after_secs ?? 0;
  const [retryAfterRemaining, setRetryAfterRemaining] = useState(serverRetryAfter);
  const { open: openSource } = useSourceOpener();
  useEffect(() => {
    if (serverRetryAfter <= 0) {
      setRetryAfterRemaining(0);
      return;
    }
    const deadline = Date.now() + serverRetryAfter * 1000;
    const update = () => {
      setRetryAfterRemaining(Math.max(0, Math.ceil((deadline - Date.now()) / 1000)));
    };
    update();
    const timer = window.setInterval(() => {
      update();
    }, 1000);
    return () => window.clearInterval(timer);
  }, [message.id, serverRetryAfter]);
  /** 正文角标的编号空间 = 本轮**检索全集**：检索先于生成到达，故流式期间角标就能点
   *  （PRD-16-006）。底部芯片行用的是 `cited`——被回答真正引用的那个子集。 */
  const markerSources = message.retrieval?.sources ?? message.sources ?? [];
  const cited = message.sources ?? [];
  // 分段为空 = 边界未知（Phase 10 之前的旧归档）：原样渲染正文，既不冒充有依据分段，
  // 也不把它改判成「模型补充」。
  const renderedContent =
    message.answerMeta && message.answerMeta.segments.length > 0
      ? message.answerMeta.segments
          .map((segment) =>
            segment.kind === "model_supplement"
              ? t("sources.model_supplement_section", {
                  label: t("sources.model_supplement"),
                  content: segment.markdown,
                })
              : segment.markdown,
          )
          .join("")
      : message.content;

  const copy = useCallback(async () => {
    await onCopy(message.content);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1600);
  }, [message.content, onCopy]);

  /**
   * 消息右键菜单（PRD-07-059）：把行内那排小图标按钮的能力**完整**给一遍，
   * 并补上只有右键才顺手的两项——「复制所选」与「引用到输入框」。
   */
  function openMessageMenu(event: ReactMouseEvent<HTMLElement>) {
    const selected = window.getSelection()?.toString() ?? "";
    const done = message.status === "done" || message.status === "cancelled";
    const items: ContextMenuItem[] = buildMenu([
      selected.trim().length > 0 && {
        kind: "action",
        id: "copySelection",
        label: tCommon("menu.copy"),
        icon: <CopyIcon size={13} />,
        onSelect: () => void onCopy(selected),
      },
      {
        kind: "action",
        id: "copy",
        label: t("action.copy_message"),
        icon: <CopyIcon size={13} />,
        onSelect: () => void copy(),
      },
      {
        kind: "action",
        id: "quote",
        label: t("action.quote"),
        icon: <QuoteIcon size={13} />,
        onSelect: () => onQuote(message.content),
      },
      separator("edit"),
      message.role === "assistant" &&
        isLast && {
          kind: "action",
          id: "regenerate",
          label: t("action.regenerate"),
          icon: <ArrowClockwiseIcon size={13} />,
          disabled: isRunning,
          onSelect: onRegenerate,
        },
      message.role === "assistant" &&
        done && {
          kind: "action",
          id: "save",
          label: t("action.save_message"),
          icon: <NotebookIcon size={13} />,
          onSelect: () => void onSave(message.content),
        },
      message.role === "assistant" &&
        done &&
        cited.length > 0 && {
          kind: "action",
          id: "saveKnowledge",
          label: t("action.save_knowledge"),
          icon: <BookOpenIcon size={13} />,
          onSelect: () => void onSaveAsKnowledge(),
        },
    ]);
    contextMenu.open(event, items);
  }

  if (message.role === "user") {
    return (
      <article className="luna-chat-message" data-role="user" onContextMenu={openMessageMenu}>
        <div className="luna-chat-message__bubble">{message.content}</div>
        <div className="luna-chat-message__actions">
          <IconButton
            density="sm"
            label={t("action.copy")}
            icon={copied ? <CheckIcon size={12} /> : <CopyIcon size={12} />}
            onClick={() => void copy()}
          />
        </div>
      </article>
    );
  }

  return (
    <article
      className="luna-chat-message"
      data-role="assistant"
      data-status={message.status}
      onContextMenu={openMessageMenu}
    >
      {message.truncated ? (
        <div className="luna-chat-message__flag">{t("flag.truncated")}</div>
      ) : null}
      {message.content.length > 0 || message.status === "running" ? (
        <ChatMarkdown
          content={renderedContent}
          streaming={message.status === "running"}
          renderText={
            markerSources.length > 0 ? citationTextRenderer(markerSources, openSource, t) : undefined
          }
        />
      ) : null}
      {/* 条件不能只看 `retrieval`：透明信息是本轮运行态、从不入档，而出处芯片是
          `PRD-16-011` 要求「重新载入后恢复」的事实。只看它，历史会话里芯片永远挂不上。 */}
      {message.retrieval || cited.length > 0 ? (
        <ChatSources
          retrieval={message.retrieval ?? null}
          cited={cited}
          onReask={onReask}
          onOpen={openSource}
          busy={isRunning}
        />
      ) : null}
      {message.status === "cancelled" ? (
        <div className="luna-chat-message__flag" data-kind="stopped">
          {t("flag.stopped")}
        </div>
      ) : null}
      {failure ? (
        <FailureCard
          code={failure.code}
          detail={
            retryAfterRemaining > 0
              ? tCommon("failure.retry_after", { seconds: retryAfterRemaining })
              : undefined
          }
          retry={
            failure.retriable && isLast
              ? {
                  label:
                    retryAfterRemaining > 0
                      ? t("action.retry_in", { seconds: retryAfterRemaining })
                      : t("action.retry"),
                  disabled: isRunning || retryAfterRemaining > 0,
                  onClick: onRegenerate,
                }
              : undefined
          }
        />
      ) : null}
      {message.status === "done" || message.status === "cancelled" ? (
        <div className="luna-chat-message__actions">
          <IconButton
            density="sm"
            label={t("action.copy")}
            icon={copied ? <CheckIcon size={12} /> : <CopyIcon size={12} />}
            onClick={() => void copy()}
          />
          {/* 悬停动作只露「复制」，其余进「⋯」（ADR-137 H7）：此前一条回答下面并排四枚同权重图标。 */}
          <HudActionMenu
            label={t("action.more")}
            density="sm"
            align="start"
            side="bottom"
            items={[
              ...(isLast
                ? [
                    {
                      id: "regenerate",
                      label: t("action.regenerate"),
                      icon: <ArrowClockwiseIcon size={14} />,
                      disabled: isRunning,
                      onSelect: onRegenerate,
                    },
                  ]
                : []),
              {
                id: "save",
                label: t("action.save_message"),
                icon: <NotebookIcon size={14} />,
                onSelect: () => void onSave(message.content),
              },
              ...(cited.length > 0
                ? [
                    {
                      id: "knowledge",
                      label: t("action.save_knowledge"),
                      icon: <BookOpenIcon size={14} />,
                      onSelect: () => void onSaveAsKnowledge(),
                    },
                  ]
                : []),
            ]}
          />
        </div>
      ) : null}
    </article>
  );
}
