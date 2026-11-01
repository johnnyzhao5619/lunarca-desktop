import { describe, expect, it } from "vitest";
import {
  canLoad,
  emptyPagedList,
  loadFailed,
  loadStarted,
  pageLoaded,
  pageOf,
  type PagedListState,
} from "./paged-list";

interface Row {
  path: string;
  cursor: string | null;
}

const row = (path: string, cursor: string | null = null): Row => ({ path, cursor });

describe("分页列表状态", () => {
  it("首页替换、续页追加——顺序恒为 Rust 给的顺序", () => {
    let state: PagedListState<Row> = emptyPagedList<Row>();
    state = pageLoaded(loadStarted(state), "first", { items: [row("a"), row("b")], cursor: "c1" });
    expect(state.items.map((i) => i.path)).toEqual(["a", "b"]);
    expect(state.exhausted).toBe(false);

    state = pageLoaded(loadStarted(state), "more", { items: [row("c")], cursor: null });
    expect(state.items.map((i) => i.path)).toEqual(["a", "b", "c"]);
    expect(state.exhausted).toBe(true);

    // 再取一次首页 = 整份替换，而不是又接一遍。
    state = pageLoaded(loadStarted(state), "first", { items: [row("z")], cursor: null });
    expect(state.items.map((i) => i.path)).toEqual(["z"]);
  });

  /**
   * 「到底了」由 Rust 说（游标 `null`），不由「这一页取满没取满」反推——那个反推要求
   * 前端知道页大小，而页大小恰恰是这次要收敛成单一常量的东西。
   */
  it("恰好取满最后一页时也报「已到底」，不白跑一次空请求", () => {
    const full = pageLoaded(emptyPagedList<Row>(), "first", {
      items: [row("a"), row("b")],
      cursor: null,
    });
    expect(full.exhausted).toBe(true);
    expect(canLoad(full, "more")).toBe(false);
  });

  it("取数中不重复发请求；没有下一页时「加载更多」是空操作", () => {
    const loading = loadStarted(emptyPagedList<Row>());
    expect(canLoad(loading, "first")).toBe(false);
    expect(canLoad(loading, "more")).toBe(false);

    const idle = pageLoaded(emptyPagedList<Row>(), "first", { items: [row("a")], cursor: "c1" });
    expect(canLoad(idle, "more")).toBe(true);
    expect(canLoad(idle, "first")).toBe(true);
  });

  it("首次取数之前不算「已到底」——那时还什么都不知道", () => {
    expect(emptyPagedList<Row>().exhausted).toBe(false);
  });

  /**
   * 失败 ≠ 空列表：前者要给重试，后者要给空态。失败时游标一并丢掉——失败可能正是
   * 「这个游标不成立」（`LIST_CURSOR_INVALID`），留着它只会让每次重试都撞同一堵墙。
   */
  it("失败保留已加载条目、丢掉游标，并与空列表区分开", () => {
    const loaded = pageLoaded(emptyPagedList<Row>(), "first", { items: [row("a")], cursor: "c1" });
    const failed = loadFailed(loadStarted(loaded));
    expect(failed.failed).toBe(true);
    expect(failed.loading).toBe(false);
    expect(failed.items.map((i) => i.path)).toEqual(["a"]);
    expect(failed.cursor).toBeNull();
    expect(failed.exhausted).toBe(false);
    // 重试即从第一页重来。
    expect(canLoad(failed, "first")).toBe(true);
    expect(canLoad(failed, "more")).toBe(false);
  });

  it("重新取数时保留旧条目，刷新不闪白", () => {
    const loaded = pageLoaded(emptyPagedList<Row>(), "first", { items: [row("a")], cursor: "c1" });
    expect(loadStarted(loaded).items.map((i) => i.path)).toEqual(["a"]);
  });
});

describe("末行游标", () => {
  /** Rust 只把游标挂在本页末行（返回是裸数组，套信封会连累两个 top-N 消费者）。 */
  it("取本页末行的游标；空页 = 没有下一页", () => {
    expect(pageOf([row("a"), row("b", "c1")]).cursor).toBe("c1");
    expect(pageOf([row("a"), row("b", null)]).cursor).toBeNull();
    expect(pageOf<Row>([]).cursor).toBeNull();
  });
});
