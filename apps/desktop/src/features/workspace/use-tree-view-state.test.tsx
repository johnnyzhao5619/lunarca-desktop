// @vitest-environment happy-dom
/**
 * 文件树排序与展开的设备状态（PRD-06-006、ADR-140）：读回之前不写、读回晚到不覆盖用户、
 * 写入串行且最后落盘的是最新状态。
 */
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import {
  commandMock,
  resetBindingsDouble,
  setCommandReply,
} from "../../ipc/bindings-test-double";
import { useTreeViewState, type TreeViewState } from "./use-tree-view-state";

let api: TreeViewState | null = null;

function Probe() {
  api = useTreeViewState();
  return null;
}

const flush = () => act(async () => {});
const ok = (data: unknown) => ({ status: "ok", data });

afterEach(() => {
  cleanup();
  resetBindingsDouble();
  api = null;
});

describe("useTreeViewState", () => {
  it("还原已存的排序与展开，读回之前不写", async () => {
    let resolve: (value: unknown) => void = () => {};
    setCommandReply("treeViewState", () => new Promise((r) => (resolve = r)));
    render(<Probe />);
    expect(commandMock("setTreeViewState")).not.toHaveBeenCalled();
    resolve(ok({ sort: "created", expanded: ["甲"] }));
    await flush();
    expect(api?.sortMode).toBe("created");
    expect([...(api?.expandedDirs ?? [])]).toEqual(["甲"]);
  });

  it("读回晚到：排序以用户改过的为准，展开集合与此刻已展开的取并集", async () => {
    let resolve: (value: unknown) => void = () => {};
    setCommandReply("treeViewState", () => new Promise((r) => (resolve = r)));
    render(<Probe />);
    // 挂载时文件树自动展开当前笔记所在的文件夹（程序性展开，不是用户的选择）。
    act(() => api?.setExpandedDirs(new Set(["项目"])));
    act(() => api?.setSortMode("mtime"));
    resolve(ok({ sort: "created", expanded: ["日记"] }));
    await flush();
    expect(api?.sortMode).toBe("mtime");
    expect([...(api?.expandedDirs ?? [])].sort()).toEqual(["日记", "项目"]);
    expect(commandMock("setTreeViewState")).toHaveBeenLastCalledWith({
      sort: "mtime",
      expanded: expect.arrayContaining(["日记", "项目"]),
    });
  });

  it("连续变更串行写入，最后一次写的是最新状态", async () => {
    setCommandReply("treeViewState", ok({ sort: "name", expanded: [] }));
    render(<Probe />);
    await flush();
    act(() => api?.setExpandedDirs(new Set(["甲"])));
    act(() => api?.setExpandedDirs((current) => new Set([...current, "乙"])));
    act(() => api?.setSortMode("created"));
    await flush();
    expect(commandMock("setTreeViewState")).toHaveBeenLastCalledWith({
      sort: "created",
      expanded: ["甲", "乙"],
    });
  });
});
