/**
 * 内容列表的分页取数控制器（TDD-07 §4.1a）。
 *
 * 这些列表的语义是**符合条件的全部条目**，不是「最近 N 条」。取数整体归 Rust：排序键、
 * 名称筛选与游标都随查询下发，前端**不得**在已加载集合上再排序或再筛选——那只是对已到手
 * 的几页重排，用户看到的顺序会随「加载了几页」而变。
 *
 * **页大小不在这里**：它是 Rust 的单一常量（`luna_vault::LIST_PAGE_SIZE`），请求把
 * `limit` 留空即「按它取一页」。本模块连读都不读它——两处各写一个数字，就是这次要修掉的那个
 * 固定上限的翻版。
 *
 * **「到底了」由 Rust 说**（游标为 `null`），不由「这一页有没有取满」反推：那个反推恰恰
 * 要求前端知道页大小，而且在「最后一页恰好取满」时会让界面白跑一次空请求。
 *
 * 纯状态迁移住在本文件上半段（无头测试锁定），下半段的 hook 只负责把它接到 IPC 上。
 */

import { useCallback, useEffect, useRef, useState } from "react";

/** 一页取数的结果。`cursor` = 下一页游标，`null` = 已遍历到底。 */
export interface PagedListPage<T> {
  items: readonly T[];
  cursor: string | null;
}

/** 一份列表的取数状态。 */
export interface PagedListState<T> {
  /** 已加载的全部条目（按 Rust 给的顺序拼接，前端不重排）。 */
  items: T[];
  /** 下一页游标；`null` = 没有下一页。**不透明**：只回传，不解析。 */
  cursor: string | null;
  loading: boolean;
  /** 已遍历到底（首次取数之前恒为 `false`——那时还什么都不知道）。 */
  exhausted: boolean;
  /** 上一次取数失败。区分于「空列表」：前者要给重试，后者要给空态。 */
  failed: boolean;
}

/** 取数模式：`first` 丢掉手上的一切从第一页重来，`more` 沿游标续一页。 */
export type PagedListMode = "first" | "more";

export function emptyPagedList<T>(): PagedListState<T> {
  return { items: [], cursor: null, loading: false, exhausted: false, failed: false };
}

/** 取数开始。`first` 保留旧条目**不清空**——清空会让刷新时整份列表闪一下白。 */
export function loadStarted<T>(state: PagedListState<T>): PagedListState<T> {
  return { ...state, loading: true, failed: false };
}

/** 一页到手。 */
export function pageLoaded<T>(
  state: PagedListState<T>,
  mode: PagedListMode,
  page: PagedListPage<T>,
): PagedListState<T> {
  return {
    items: mode === "first" ? [...page.items] : [...state.items, ...page.items],
    cursor: page.cursor,
    loading: false,
    exhausted: page.cursor === null,
    failed: false,
  };
}

/**
 * 取数失败。
 *
 * **游标一并丢掉**：失败可能正是「这个游标不成立」（`LIST_CURSOR_INVALID`，例如排序键
 * 已经换了），留着它只会让每一次重试都撞同一堵墙。丢掉之后重试即从第一页重来。
 */
export function loadFailed<T>(state: PagedListState<T>): PagedListState<T> {
  return { ...state, cursor: null, loading: false, exhausted: false, failed: true };
}

/** 该不该发出这次请求（`more` 在没有下一页或正在取数时不发）。 */
export function canLoad<T>(state: PagedListState<T>, mode: PagedListMode): boolean {
  if (state.loading) {
    return false;
  }
  return mode === "first" || state.cursor !== null;
}

/**
 * 从一页命中里取出「下一页游标」。
 *
 * Rust 只把游标挂在**本页末行**（返回是裸数组，套信封会连累两个 top-N 消费者）；
 * 收成一句话是为了让各调用点不各写一遍 `at(-1)`。
 */
export function pageOf<T extends { cursor: string | null }>(items: T[]): PagedListPage<T> {
  return { items, cursor: items.length > 0 ? (items[items.length - 1]?.cursor ?? null) : null };
}

/** 组件侧看到的一份分页列表。 */
export interface PagedList<T> extends PagedListState<T> {
  /** 续一页（没有下一页 / 正在取数时是空操作）。 */
  loadMore: () => void;
  /** 从第一页重来（取数口径变了或用户重试）。 */
  reload: () => Promise<void>;
}

export interface PagedListOptions<T> {
  /**
   * 取一页。`null` = 取数失败（调用方已把 Rust 的错误码翻译成"这次没成"）。
   *
   * **游标原样交给它、原样传回去**：前端不解析、不自行构造。
   */
  fetchPage: (cursor: string | null) => Promise<PagedListPage<T> | null>;
  /**
   * **取数口径的身份**：排序键、筛选词、库版本等任何会改变"这份列表是什么"的东西。
   * 它一变就丢掉游标从第一页重来——拿旧口径的游标去续新口径的列表，Rust 会当场判
   * `LIST_CURSOR_INVALID`（排序键不匹配那一档），而没有这一条时它只是静默漏掉一段。
   */
  key: string;
  /** 为假时不取数（如 Vault 尚未打开）。 */
  enabled?: boolean;
}

export function usePagedList<T>({ fetchPage, key, enabled = true }: PagedListOptions<T>): PagedList<T> {
  const [state, setState] = useState<PagedListState<T>>(emptyPagedList<T>);
  // 取数函数每次渲染都是新的闭包；放进 ref 让 `run` 不必随它重建。
  const fetchRef = useRef(fetchPage);
  fetchRef.current = fetchPage;
  // 迟到的响应不得覆盖新状态：口径变了、或用户又点了一次「加载更多」时，上一次请求作废。
  const runRef = useRef(0);
  const stateRef = useRef(state);

  const run = useCallback(async (mode: PagedListMode) => {
    if (!canLoad(stateRef.current, mode)) {
      return;
    }
    const cursor = mode === "first" ? null : stateRef.current.cursor;
    const token = ++runRef.current;
    stateRef.current = loadStarted(stateRef.current);
    setState(stateRef.current);
    const page = await fetchRef.current(cursor);
    if (token !== runRef.current) {
      return;
    }
    stateRef.current = page
      ? pageLoaded(stateRef.current, mode, page)
      : loadFailed(stateRef.current);
    setState(stateRef.current);
  }, []);

  /**
   * 作废在途请求并从第一页重来。
   *
   * **旧条目留在屏幕上**（只丢游标与"到底了"），刷新时不闪白；新的一页到手时整份替换。
   */
  const restart = useCallback(() => {
    runRef.current += 1;
    stateRef.current = { ...emptyPagedList<T>(), items: stateRef.current.items };
    return run("first");
  }, [run]);

  useEffect(() => {
    if (!enabled) {
      // Vault 关闭/切换过渡期：作废在途响应并清掉旧库投影。只是不继续请求会让旧请求在
      // disabled 期间迟到落地，把上一库的条目重新挂回界面。
      runRef.current += 1;
      stateRef.current = emptyPagedList<T>();
      setState(stateRef.current);
      return;
    }
    void restart();
  }, [key, enabled, restart]);

  return {
    ...state,
    loadMore: () => void run("more"),
    reload: restart,
  };
}
