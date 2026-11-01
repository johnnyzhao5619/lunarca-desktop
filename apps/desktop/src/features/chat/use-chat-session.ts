/**
 * Chat 会话：挂载恢复（`chat_attach`）+ 流式收发 + 跨视图同步（`chat://changed`）。
 *
 * 会话唯一事实源在 Rust 内存（PRD-03-012）；这里只维护**渲染投影**：已落账消息 +
 * 进行中一轮的流式缓冲 + 终局状态（失败卡/已停止）。视图卸载重挂不丢会话；同 scope
 * 其它视图的终局经 typed event 触发重新 attach。
 *
 * **投影放在共享源里，不放在宿主的 state 里**（D1）：流式回答一个 token 换一次消息列表，
 * 而持有会话的是工作区根（主侧栏历史与主区对话共用一份）。投影若是根的 state，聊天时每个
 * token 都把整个工作区重渲染一遍——离开对话页去看笔记时回答还在流，编辑器也跟着一拍一渲。
 * 现在宿主只持有这份源（引用恒定），对话视图整份订阅，根与侧栏经 `useChatSessionSelector`
 * 只取「当前链接的是哪一篇」这类粗值。
 */

import { Channel } from "@tauri-apps/api/core";
import { useMemo, useSyncExternalStore } from "react";
import { useStoreSelector, type SelectionEquality } from "../../hooks/use-store-selector";
import {
  commands,
  events,
  type ChatEvent,
  type AnswerMetaDto,
  type ChatFailureDto,
  type ChatScope,
  type ChatSnapshotDto,
  type MentionScopeDto,
  type RetrievalInfoDto,
  type RetrievalSourceDto,
} from "../../ipc/bindings";

export type ChatMessageStatus = "done" | "running" | "failed" | "cancelled";

export interface ChatUiMessage {
  id: string;
  role: "user" | "assistant";
  content: string;
  status: ChatMessageStatus;
  /** 结构化失败（status === "failed" 时存在）。 */
  failure?: ChatFailureDto;
  /** 本轮上下文发生截断（PRD-03 §5 显式提示）。 */
  truncated?: boolean;
  /** 本轮的库内检索透明信息（PRD-16-006；未检索时缺省）。 */
  retrieval?: RetrievalInfoDto;
  /** 本条回答实际引用的出处（PRD-16-005）。 */
  sources?: RetrievalSourceDto[];
  /** Rust 终局依据判定；未检索的普通对话保持缺省。 */
  answerMeta?: AnswerMetaDto;
}

/**
 * 检索透明行上的**一次性**覆盖（PRD-16-006/008/041）：一次查询有效、不改设置，
 * 由 Rust 在该轮成功收尾时复位。
 *
 * 三项一起传而不是各给一个动作：透明行上的每一次调整都要**整体重答**，分开传就会
 * 出现"勾了排除记录、顺手又剔一篇，结果第一项被上一次的旧值覆盖回去"。这三项是同一个
 * 快照的三个字段，不是三条独立指令。
 */
export interface ReaskOverrides {
  /** 临时纳入对话笔记（PRD-16-008，默认排除的口径翻转）。 */
  includeChat: boolean;
  /** 整组排除记录态（PRD-16-041，方向相反的同一种翻转）。 */
  excludeDrafts: boolean;
  /** 本次剔除的笔记路径（PRD-16-006）。 */
  excluded: string[];
}

/** 会话的渲染投影（不可变快照：每次变化换一份新的）。 */
export interface ChatSessionSnapshot {
  messages: ChatUiMessage[];
  /** 本视图是否有进行中的生成。 */
  isRunning: boolean;
  /** **临时对话**（PRD-03-012）：本会话不落盘/不进历史/退出即弃。 */
  temporary: boolean;
  presetId: string | null;
  /** 已链接的归档笔记路径（历史列表高亮当前会话，PRD-03-013）。 */
  notePath: string | null;
  /** 本会话是否检索知识库（PRD-16-005 输入区芯片的真值）。 */
  grounding: boolean;
}

/** 会话动作（引用恒定：随共享源一起建一次）。 */
export interface ChatSessionActions {
  /** 切换本会话的知识库开关。 */
  setGrounding: (enabled: boolean) => Promise<void>;
  /** 换一批检索依据重答（PRD-16-006/008/041 透明行的三个动作共用）。 */
  reask: (overrides: ReaskOverrides) => Promise<void>;
  /**
   * 发一条带 `@` 定范围的消息（PRD-16-009）。
   *
   * 范围随**这一条消息**走：在提交之前先设一次性覆盖项，Rust 在该轮成功收尾时复位。
   * 与 `send` 分开而不是加个可选参数——`send` 的调用点还有语音落地等路径，
   * 让它们都去回答"这次的范围是什么"是把一个它们不关心的问题摊给每一处。
   */
  sendWithScope: (message: string, mentions: MentionScopeDto[]) => Promise<void>;
  /** 把某条回答存成知识笔记（洞见回流，PRD-16-010）；成功返回笔记路径。 */
  saveMessageToNote: (messageIndex: number) => Promise<string | null>;
  send: (message: string) => Promise<void>;
  regenerate: () => Promise<void>;
  stop: () => Promise<void>;
  /** 新对话（可带角色预设，PRD-03-010）。`temporary` 覆盖持久性，`null`=跟随设置默认。 */
  reset: (presetId: string | null, temporary?: boolean | null) => Promise<void>;
  /** 载入一条历史会话并链接（PRD-03-013）。 */
  openNote: (path: string) => Promise<boolean>;
  /** 整段会话存入笔记；成功返回笔记路径。 */
  saveToNote: () => Promise<string | null>;
}

/** 对话视图拿到的整份会话：投影 + 动作。 */
export type ChatSessionState = ChatSessionSnapshot & ChatSessionActions;

/** 一个 scope 的会话共享源（宿主建一次，引用恒定）。 */
export interface ChatSessionStore extends ChatSessionActions {
  subscribe: (onChange: () => void) => () => void;
  getSnapshot: () => ChatSessionSnapshot;
  /** 与 Rust 对齐：立即 attach，并订阅换库与同 scope 终局两条广播；返回解除订阅。 */
  connect: () => () => void;
}

function nextLocalId(): string {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) {
    return crypto.randomUUID();
  }
  return `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
}

const INITIAL_SNAPSHOT: ChatSessionSnapshot = {
  messages: [],
  isRunning: false,
  temporary: false,
  presetId: null,
  notePath: null,
  grounding: true,
};

export function createChatSessionStore(scope: ChatScope): ChatSessionStore {
  let snapshot = INITIAL_SNAPSHOT;
  const listeners = new Set<() => void>();
  /** 本视图**自己发起**的 turn 在飞（区别于 attach 发现的他视图 busy）。 */
  let localTurn = false;
  /** 本视图发起的 turn 正在/刚刚结束：吞掉自己触发的 `chat://changed`，保留本地终局呈现
   *（失败卡不落账，重新 attach 会把它冲掉）。 */
  let suppressChanged = false;

  /** 换一份快照并通知订阅者（同一次改动里的几个字段一起换，订阅者只渲染一次）。 */
  const patch = (next: Partial<ChatSessionSnapshot>) => {
    snapshot = { ...snapshot, ...next };
    for (const listener of listeners) listener();
  };

  const mapMessages = (update: (message: ChatUiMessage) => ChatUiMessage) => {
    patch({ messages: snapshot.messages.map(update) });
  };

  const updateMessage = (id: string, change: Partial<ChatUiMessage>) => {
    mapMessages((message) => (message.id === id ? { ...message, ...change } : message));
  };

  const appendToken = (id: string, token: string) => {
    mapMessages((message) =>
      message.id === id ? { ...message, content: message.content + token } : message,
    );
  };

  /** 快照 → 本地渲染投影（attach / reset / openNote 共用）。 */
  const applySnapshot = (dto: ChatSnapshotDto) => {
    patch({
      messages: dto.messages.map((message) => ({
        id: nextLocalId(),
        role: message.role === "user" ? "user" : "assistant",
        content: message.content,
        status: "done" as const,
        sources: message.sources.length > 0 ? message.sources : undefined,
        answerMeta: message.answer_meta ?? undefined,
      })),
      isRunning: dto.busy,
      temporary: dto.temporary,
      presetId: dto.preset_id,
      notePath: dto.note_path,
      grounding: dto.grounding,
    });
  };

  const attach = async () => {
    const result = await commands.chatAttach(scope);
    if (result.status === "ok") {
      applySnapshot(result.data);
    }
  };

  const connect = () => {
    void attach();
    // 库已切换（ADR-025）：Rust 侧会话按「顶替」清空，但本源可能活在**独立 webview**
    // （快捷对话窗）里、不随主窗口重挂载——不重新对齐就还显示着上一个库的会话内容。
    const unlistenVault = events.vaultSwitched.listen(() => {
      localTurn = false;
      void attach();
    });
    // 同 scope 其它视图（或本视图卸载期间）的终局 → 重新 attach 同步真值。
    const unlistenChanged = events.chatChanged.listen((event) => {
      if (event.payload.scope !== scope) {
        return;
      }
      // 归档链接无条件同步（本视图自己的 turn 也可能在后端建立/更新自动归档链接）。
      if (snapshot.notePath !== event.payload.note_path) {
        patch({ notePath: event.payload.note_path });
      }
      if (suppressChanged) {
        suppressChanged = false;
        return;
      }
      if (!localTurn) {
        void attach();
      }
    });
    return () => {
      void unlistenVault.then((fn) => fn());
      void unlistenChanged.then((fn) => fn());
    };
  };

  /** 一轮生成的共同事件循环（发送/重新生成/重试共用）。 */
  const runTurn = async (
    assistantId: string,
    invoke: (channel: Channel<ChatEvent>) => Promise<{ status: "ok" } | { status: "error"; error: string }>,
  ) => {
    if (snapshot.isRunning) {
      return;
    }
    localTurn = true;
    patch({ isRunning: true });
    suppressChanged = true;
    try {
      const terminal = new Promise<void>((resolve) => {
        const channel = new Channel<ChatEvent>();
        channel.onmessage = (event) => {
          switch (event.kind) {
            case "started":
              break;
            case "truncated":
              updateMessage(assistantId, { truncated: true });
              break;
            case "retrieval":
              // 检索先于生成到达，故引用角标不必等流式结束即可点击。
              updateMessage(assistantId, { retrieval: event.data });
              break;
            case "token":
              appendToken(assistantId, event.data);
              break;
            case "done":
              updateMessage(assistantId, {
                content: event.data.text,
                status: "done",
                sources: event.data.sources.length > 0 ? event.data.sources : undefined,
                answerMeta: event.data.answer_meta ?? undefined,
              });
              resolve();
              break;
            case "cancelled":
              updateMessage(assistantId, { content: event.data, status: "cancelled" });
              resolve();
              break;
            case "failed":
              updateMessage(assistantId, { status: "failed", failure: event.data });
              resolve();
              break;
          }
        };
        void invoke(channel).then((result) => {
          if (result.status === "error") {
            updateMessage(assistantId, {
              status: "failed",
              failure: { code: "COMMAND_FAILED", retriable: true, retry_after_secs: null },
            });
            resolve();
          }
        });
      });
      await terminal;
    } finally {
      localTurn = false;
      patch({ isRunning: false });
      // 兜底清除：命令层出错时终局广播不会到达，避免 suppress 残留吞掉下一次外部变更。
      window.setTimeout(() => {
        suppressChanged = false;
      }, 1000);
    }
  };

  const send = async (message: string) => {
    const content = message.trim();
    if (!content || snapshot.isRunning) {
      return;
    }
    const assistantId = nextLocalId();
    patch({
      messages: [
        ...snapshot.messages,
        { id: nextLocalId(), role: "user", content, status: "done" },
        { id: assistantId, role: "assistant", content: "", status: "running" },
      ],
    });
    await runTurn(assistantId, (channel) => commands.chatSend(scope, content, channel));
  };

  const regenerate = async () => {
    if (snapshot.isRunning) {
      return;
    }
    // 复用末条 assistant 占位（失败卡重试同路径）；无 assistant 占位（上轮 send 失败后被
    // 用户丢弃等场景不存在——失败占位保留）时不动作。
    const last = [...snapshot.messages].reverse().find((message) => message.role === "assistant");
    if (!last) {
      return;
    }
    updateMessage(last.id, {
      content: "",
      status: "running",
      failure: undefined,
      truncated: undefined,
      // 上一版的检索与出处一并清掉：重答可能换一批依据（PRD-16-006 剔除后重答），
      // 陈旧的透明行挂在新回答上是谎报（新 Retrieval 事件到达前那几百毫秒也不行）。
      retrieval: undefined,
      sources: undefined,
    });
    await runTurn(last.id, (channel) => commands.chatRegenerate(scope, channel));
  };

  const stop = async () => {
    await commands.chatStop(scope);
  };

  const reset = async (nextPresetId: string | null, nextTemporary: boolean | null = null) => {
    const result = await commands.chatReset(scope, nextPresetId, nextTemporary);
    if (result.status === "ok") {
      localTurn = false;
      applySnapshot(result.data);
    }
  };

  const openNote = async (path: string) => {
    const result = await commands.chatOpenNote(scope, path);
    if (result.status === "ok") {
      localTurn = false;
      applySnapshot(result.data);
      return true;
    }
    return false;
  };

  const setGrounding = async (enabled: boolean) => {
    const result = await commands.chatSetGrounding(scope, enabled);
    if (result.status === "ok") {
      patch({ grounding: enabled });
    }
  };

  /** 换一批检索依据重答：设覆盖项 → 重新生成（覆盖项由 Rust 在该轮收尾时复位）。 */
  const reask = async ({ includeChat, excludeDrafts, excluded }: ReaskOverrides) => {
    if (snapshot.isRunning) {
      return;
    }
    const result = await commands.chatSetRetrievalOverrides(
      scope,
      includeChat,
      excludeDrafts,
      excluded,
      [],
    );
    if (result.status !== "ok") {
      return;
    }
    await regenerate();
  };

  const sendWithScope = async (message: string, mentions: MentionScopeDto[]) => {
    if (snapshot.isRunning) {
      return;
    }
    // **空 mentions 也要同步**（验收轮缺陷）：覆盖项只在成功走完的那一轮复位，
    // 失败/取消会保留（给「重试」沿用）。若上一轮带 @ 失败后用户改问**新问题**，
    // 不同步就等于旧范围静默作用于新问题——而芯片 UI 已经清空，界面在撒谎。
    // 「发新消息即复位」是 RetrievalOverrides 的既定契约，这里是它的执行点。
    // 形态解析在 Rust（认不出的一律丢弃）；设失败就照常发——范围收窄不了
    // 远好过整条消息发不出去。
    await commands.chatSetRetrievalOverrides(scope, false, false, [], mentions);
    await send(message);
  };

  const saveMessageToNote = async (messageIndex: number) => {
    const result = await commands.chatSaveMessageToNote(scope, messageIndex);
    return result.status === "ok" ? result.data.path : null;
  };

  const saveToNote = async () => {
    const result = await commands.chatSaveToNote(scope);
    if (result.status === "ok") {
      // 「保存此对话」后端会把会话转持久 → 本地同步退出临时态、链接笔记。
      patch({ temporary: false, notePath: result.data.path });
      return result.data.path;
    }
    return null;
  };

  return {
    subscribe: (onChange) => {
      listeners.add(onChange);
      return () => listeners.delete(onChange);
    },
    getSnapshot: () => snapshot,
    connect,
    send,
    regenerate,
    stop,
    reset,
    openNote,
    saveToNote,
    setGrounding,
    reask,
    sendWithScope,
    saveMessageToNote,
  };
}

/**
 * 整份会话（对话视图用）：每个 token 都重渲染调用方——只该挂在真正显示消息的那一层。
 */
export function useChatSession(store: ChatSessionStore): ChatSessionState {
  const snapshot = useSyncExternalStore(store.subscribe, store.getSnapshot);
  return useMemo(() => ({ ...store, ...snapshot }), [store, snapshot]);
}

/** 会话里的一位（工作区根 / 侧栏用）：只在选出来的值变了时重渲染调用方。 */
export function useChatSessionSelector<T>(
  store: ChatSessionStore,
  select: (snapshot: ChatSessionSnapshot) => T,
  isEqual?: SelectionEquality<T>,
): T {
  return useStoreSelector(store.subscribe, store.getSnapshot, select, isEqual);
}
