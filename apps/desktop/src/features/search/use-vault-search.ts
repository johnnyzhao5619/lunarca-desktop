// 搜索视图控制器（PRD-06-031 / PRD-06-014）。
//
// 前端零业务（ADR-003）：查询语法解析、命中定位、替换计划全在 Rust，本 Hook 只管
// **交互节奏**——防抖、请求竞态、展开/折叠、替换的两步（先预览再落盘）。
//
// 一条重要的竞态纪律：搜索是逐键触发的，晚发的请求可能先回来。用自增序号认领结果，
// 过期响应直接丢弃，否则用户会看到旧词的命中列表挂在新词下面。

import { useCallback, useEffect, useRef, useState } from "react";
import { commands, type ReplacePlanDto, type VaultSearchResultDto } from "../../ipc/bindings";
import { vaultErrorText } from "../../lib/vault-error";

/** 逐键搜索的防抖（本地 FTS 很快，这点延迟只为避免每个字符都读一遍候选文件）。 */
const SEARCH_DEBOUNCE_MS = 200;

export type ReplaceStage =
  | { kind: "idle" }
  | { kind: "preview"; plan: ReplacePlanDto }
  | { kind: "done"; plan: ReplacePlanDto };

export interface VaultSearchController {
  query: string;
  setQuery: (value: string) => void;
  matchCase: boolean;
  toggleMatchCase: () => void;
  results: VaultSearchResultDto | null;
  loading: boolean;
  error: string | null;
  /** 已折叠的笔记路径集合（默认全部展开）。 */
  collapsed: ReadonlySet<string>;
  toggleCollapsed: (path: string) => void;
  collapseAll: () => void;
  expandAll: () => void;
  replaceOpen: boolean;
  setReplaceOpen: (open: boolean) => void;
  replacement: string;
  setReplacement: (value: string) => void;
  replaceStage: ReplaceStage;
  replaceBusy: boolean;
  /** 生成替换预览（不写盘）。 */
  previewReplace: () => Promise<void>;
  /** 落盘替换；`path` 为空即作用于全部命中笔记。 */
  applyReplace: (path?: string) => Promise<void>;
  /** 丢弃替换预览（改词、改查询、关闭替换区时都要作废，否则会照着旧计划落盘）。 */
  resetReplace: () => void;
  refresh: () => void;
}

export function useVaultSearch(): VaultSearchController {
  const [query, setQueryState] = useState("");
  const [matchCase, setMatchCase] = useState(false);
  const [results, setResults] = useState<VaultSearchResultDto | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(() => new Set());
  const [replaceOpen, setReplaceOpen] = useState(false);
  const [replacement, setReplacementState] = useState("");
  const [replaceStage, setReplaceStage] = useState<ReplaceStage>({ kind: "idle" });
  const [replaceBusy, setReplaceBusy] = useState(false);
  const [reloadToken, setReloadToken] = useState(0);
  const requestSeq = useRef(0);

  const resetReplace = useCallback(() => {
    setReplaceStage({ kind: "idle" });
  }, []);

  const setQuery = useCallback(
    (value: string) => {
      setQueryState(value);
      resetReplace();
    },
    [resetReplace],
  );

  const setReplacement = useCallback(
    (value: string) => {
      setReplacementState(value);
      resetReplace();
    },
    [resetReplace],
  );

  const toggleMatchCase = useCallback(() => {
    setMatchCase((value) => !value);
    resetReplace();
  }, [resetReplace]);

  useEffect(() => {
    const trimmed = query.trim();
    if (!trimmed) {
      requestSeq.current += 1;
      setResults(null);
      setError(null);
      setLoading(false);
      return;
    }
    const seq = ++requestSeq.current;
    setLoading(true);
    const timer = window.setTimeout(() => {
      void commands.vaultSearch(trimmed, matchCase).then((result) => {
        if (seq !== requestSeq.current) {
          return;
        }
        setLoading(false);
        if (result.status === "ok") {
          setResults(result.data);
          setError(null);
          setCollapsed(new Set());
          return;
        }
        setResults(null);
        setError(vaultErrorText(result.error));
      });
    }, SEARCH_DEBOUNCE_MS);
    return () => {
      window.clearTimeout(timer);
    };
  }, [query, matchCase, reloadToken]);

  const toggleCollapsed = useCallback((path: string) => {
    setCollapsed((current) => {
      const next = new Set(current);
      if (!next.delete(path)) {
        next.add(path);
      }
      return next;
    });
  }, []);

  const collapseAll = useCallback(() => {
    setCollapsed(new Set(results?.notes.map((note) => note.path) ?? []));
  }, [results]);

  const expandAll = useCallback(() => {
    setCollapsed(new Set());
  }, []);

  const previewReplace = useCallback(async () => {
    const trimmed = query.trim();
    if (!trimmed) {
      return;
    }
    setReplaceBusy(true);
    const result = await commands.vaultReplace(trimmed, replacement, matchCase, null, true);
    setReplaceBusy(false);
    if (result.status === "ok") {
      setReplaceStage({ kind: "preview", plan: result.data });
      setError(null);
      return;
    }
    setReplaceStage({ kind: "idle" });
    setError(vaultErrorText(result.error));
  }, [query, replacement, matchCase]);

  const applyReplace = useCallback(
    async (path?: string) => {
      const trimmed = query.trim();
      if (!trimmed) {
        return;
      }
      setReplaceBusy(true);
      const result = await commands.vaultReplace(
        trimmed,
        replacement,
        matchCase,
        path ? [path] : null,
        false,
      );
      setReplaceBusy(false);
      if (result.status !== "ok") {
        setError(vaultErrorText(result.error));
        return;
      }
      setReplaceStage({ kind: "done", plan: result.data });
      setError(null);
      // 文件已变，重新检索——否则列表还挂着已经不存在的命中。
      setReloadToken((token) => token + 1);
    },
    [query, replacement, matchCase],
  );

  return {
    query,
    setQuery,
    matchCase,
    toggleMatchCase,
    results,
    loading,
    error,
    collapsed,
    toggleCollapsed,
    collapseAll,
    expandAll,
    replaceOpen,
    setReplaceOpen,
    replacement,
    setReplacement,
    replaceStage,
    replaceBusy,
    previewReplace,
    applyReplace,
    resetReplace,
    refresh: () => setReloadToken((token) => token + 1),
  };
}
