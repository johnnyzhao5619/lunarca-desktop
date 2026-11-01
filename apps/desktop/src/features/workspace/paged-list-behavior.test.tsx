// @vitest-environment happy-dom

import { act, renderHook } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { usePagedList, type PagedListPage } from "./paged-list";

interface Row {
  path: string;
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

describe("分页 hook 的异步边界", () => {
  it("口径切换后丢弃旧请求的迟到响应", async () => {
    const first = deferred<PagedListPage<Row> | null>();
    const second = deferred<PagedListPage<Row> | null>();
    const fetchPage = vi
      .fn<(cursor: string | null) => Promise<PagedListPage<Row> | null>>()
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(second.promise);
    const { result, rerender } = renderHook(
      ({ key }) => usePagedList<Row>({ key, fetchPage }),
      { initialProps: { key: "vault-a" } },
    );

    rerender({ key: "vault-b" });
    await act(async () => first.resolve({ items: [{ path: "old.md" }], cursor: null }));
    expect(result.current.items).toEqual([]);
    await act(async () => second.resolve({ items: [{ path: "new.md" }], cursor: null }));
    expect(result.current.items).toEqual([{ path: "new.md" }]);
  });

  it("禁用时清空旧库投影并阻止在途响应回填", async () => {
    const request = deferred<PagedListPage<Row> | null>();
    const fetchPage = vi.fn(() => request.promise);
    const { result, rerender } = renderHook(
      ({ enabled }) => usePagedList<Row>({ key: "vault-a", enabled, fetchPage }),
      { initialProps: { enabled: true } },
    );

    rerender({ enabled: false });
    await act(async () => request.resolve({ items: [{ path: "old.md" }], cursor: null }));
    expect(result.current.items).toEqual([]);
    expect(result.current.loading).toBe(false);
  });
});
