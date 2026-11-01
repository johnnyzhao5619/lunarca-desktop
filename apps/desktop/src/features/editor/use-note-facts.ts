// 「这一篇的对象级事实」取数口（`note_facts`，PRD-07-031）。
//
// **两格共用一次往返**：「这一篇的标签」与「笔记信息」问的是同一个对象，各拉一次
// 只是把一次往返变成两次，而两次之间那一篇可能已经被改过——两格于是会互相矛盾。
//
// 刷新口径与大纲、反链等既有面板逐条相同：`note://written`（本窗口与其它窗口的保存都
// 走它）+ `vault://changed` 兜底。不额外轮询。

import { useCallback, useEffect, useRef, useState } from "react";

import { commands, events, type NoteFactsDto } from "../../ipc/bindings";
import { useVaultReload } from "../../hooks/use-vault-reload";

export interface UseNoteFacts {
  /** `null` = 还没取到（首帧 / 取数失败）。 */
  facts: NoteFactsDto | null;
  loading: boolean;
  /** 规整后的笔记路径；`null` = 当前对象不是一篇 Markdown 笔记，两格都不该渲染内容。 */
  notePath: string | null;
}

export function useNoteFacts(path: string | null): UseNoteFacts {
  const notePath = path?.endsWith(".md") ? path : null;
  const [facts, setFacts] = useState<NoteFactsDto | null>(null);
  const [loading, setLoading] = useState(false);

  const load = useCallback(async (target: string) => {
    const res = await commands.noteFacts(target);
    return res.status === "ok" ? res.data : null;
  }, []);
  /** 最近一次重取（`useVaultReload` 恒调最新那一份）。 */
  const refreshRef = useRef<(() => void) | null>(null);


  useEffect(() => {
    if (!notePath) {
      setFacts(null);
      setLoading(false);
      return;
    }
    let disposed = false;
    async function refresh(target: string) {
      const next = await load(target);
      if (!disposed) {
        setFacts(next);
        setLoading(false);
      }
    }
    setLoading(true);
    // 换笔记时先清空：留着上一篇的标签与时间，比空着更容易被读成"这一篇就是这样"。
    setFacts(null);
    void refresh(notePath);

    // 保存后必须重取，**不能拿广播里的正文自己算**：标签是索引出来的（用户标签与
    // AI 标签同池），正文里那几个 `#tag` 只是其中一半。
    const unlistenWritten = events.noteWritten.listen((event) => {
      if (!disposed && event.payload.path === notePath) {
        void refresh(notePath);
      }
    });
    refreshRef.current = () => {
      if (!disposed) {
        void refresh(notePath);
      }
    };
    return () => {
      disposed = true;
      refreshRef.current = null;
      void unlistenWritten.then((dispose) => dispose());
    };
  }, [notePath, load]);

  // Vault 变更兜底重取（主路径是上面那条按路径过滤的 `noteWritten`）。
  useVaultReload(() => refreshRef.current?.());

  return { facts, loading, notePath };
}
