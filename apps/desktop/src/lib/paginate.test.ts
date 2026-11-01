import { describe, expect, it } from "vitest";

import { paginate } from "./paginate";

describe("paginate", () => {
  it("按页大小切分，末页可以不满", () => {
    const items = [1, 2, 3, 4, 5];
    expect(paginate(items, 0, 2)).toEqual({ items: [1, 2], page: 0, pageCount: 3 });
    expect(paginate(items, 2, 2)).toEqual({ items: [5], page: 2, pageCount: 3 });
  });

  /**
   * 条目会因为筛选、删除、刷新而变少，而用户可能正停在第 5 页。夹住的结果是「跳到最后
   * 一页」；不夹的话用户看到的是一片空白，而列表里明明还有东西。
   */
  it("页码越界夹到最后一页，而不是给出空白", () => {
    const page = paginate([1, 2, 3], 9, 2);
    expect(page.page).toBe(1);
    expect(page.items).toEqual([3]);
  });

  it("负页码夹到第一页", () => {
    expect(paginate([1, 2, 3], -4, 2).page).toBe(0);
  });

  /** 空列表仍是「第 1 / 1 页」——「第 1 / 0 页」读不通。 */
  it("空列表有一页", () => {
    expect(paginate([], 0, 20)).toEqual({ items: [], page: 0, pageCount: 1 });
  });

  /** 页大小是个笔误（0 或负数）时不能除以零。 */
  it("页大小小于 1 时按 1 处理", () => {
    expect(paginate([1, 2], 1, 0)).toEqual({ items: [2], page: 1, pageCount: 2 });
  });
});
