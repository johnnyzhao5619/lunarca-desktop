/**
 * AI 对话控制器（PRD-03）：把会话（`ChatSessionStore`，scope 键控事实源在 Rust）与历史
 * （`useChatHistory`）及其跨面板动作聚为**单一实例**，供「主侧栏历史列表」与「主区对话」
 * 共享同一会话——二者若各自建一份会话会在 openNote/reset 上分叉（ADR-003：前端只
 * 编排、不复制事实源）。控制器只暴露纯操作（返回布尔/路径），toast/聚焦等视图反馈留在
 * `ChatView`。
 *
 * 控制器**不订阅**会话投影：它只持有共享源（引用恒定），流式回答的每个 token 只重渲染
 * 经 `useChatSession` 整份订阅的对话视图；宿主要哪一位就经 `useChatSessionSelector` 取哪一位
 * （D1：此前持有会话 state 的是工作区根，聊天时每个 token 重渲染整个工作区）。
 */

import { useCallback, useEffect, useState } from "react";
import type { ChatScope } from "../../ipc/bindings";
import { useChatHistory, type ChatHistoryRemoval, type ChatHistoryState } from "./use-chat-history";
import { createChatSessionStore, type ChatSessionStore } from "./use-chat-session";

export interface ChatController {
  store: ChatSessionStore;
  history: ChatHistoryState;
  /** 新对话（保持/切换角色预设，PRD-03-010）。 */
  startPresetChat: (presetId: string | null) => Promise<void>;
  /** 载入一条历史会话并链接（PRD-03-013）；返回是否成功。 */
  openHistoryEntry: (path: string) => Promise<boolean>;
  /** 删除历史会话；删的是当前会话则顺带开新会话。返回是否成功。 */
  deleteHistoryEntries: (paths: readonly string[]) => Promise<ChatHistoryRemoval>;
}

export function useChatController(scope: ChatScope): ChatController {
  const [store] = useState(() => createChatSessionStore(scope));
  useEffect(() => store.connect(), [store]);
  const history = useChatHistory();

  const startPresetChat = useCallback(
    (presetId: string | null) => store.reset(presetId),
    [store],
  );

  const openHistoryEntry = useCallback((path: string) => store.openNote(path), [store]);

  const deleteHistoryEntries = useCallback(
    async (paths: readonly string[]) => {
      const active = store.getSnapshot().notePath;
      const removal = await history.remove(paths);
      // 正开着的那一篇被删掉了就换成新对话——否则界面停在一篇已经不在库里的会话上。
      if (active !== null && removal.deleted.includes(active)) {
        await store.reset(store.getSnapshot().presetId);
      }
      return removal;
    },
    [history, store],
  );

  return { store, history, startPresetChat, openHistoryEntry, deleteHistoryEntries };
}
