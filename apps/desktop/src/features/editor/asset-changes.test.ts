// @vitest-environment happy-dom
/**
 * 库外附件变化（PRD-06-012 ③，V2b）：只有附件变化推进代际号（自动保存每次也发
 * `vault://changed`，笔记变化若也触发，正文里的图会随每次保存重新解析、闪一下）；
 * 一阵变化合并成一次；图片部件代际号不同就不复用旧 DOM，加载失败显示成断链。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { emitEvent, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { currentAssetGeneration, onAssetsChanged } from "../media/asset-changes";
import { ImageEmbedWidget } from "./image-embed";

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
  resetBindingsDouble();
});

describe("附件代际号", () => {
  it("只对附件变化推进，一阵变化只推进一次", () => {
    const seen = vi.fn();
    const stop = onAssetsChanged(seen);
    const before = currentAssetGeneration();
    emitEvent("vaultChanged", { rel_path: "笔记/甲.md", deleted: false });
    vi.advanceTimersByTime(1000);
    expect(currentAssetGeneration()).toBe(before);
    emitEvent("vaultChanged", { rel_path: "笔记/assets/图.png", deleted: true });
    emitEvent("vaultChanged", { rel_path: "笔记/assets/图2.png", deleted: true });
    vi.advanceTimersByTime(1000);
    expect(currentAssetGeneration()).toBe(before + 1);
    expect(seen).toHaveBeenCalledTimes(1);
    stop();
  });
});

describe("ImageEmbedWidget", () => {
  it("代际号不同就不复用旧 DOM", () => {
    const a = new ImageEmbedWidget("甲.md", "图.png", "", null, undefined, 1);
    const b = new ImageEmbedWidget("甲.md", "图.png", "", null, undefined, 1);
    const c = new ImageEmbedWidget("甲.md", "图.png", "", null, undefined, 2);
    expect(a.eq(b)).toBe(true);
    expect(a.eq(c)).toBe(false);
  });

  it("地址解析成功但图片加载失败时显示成断链", async () => {
    setCommandReply("vaultAssetPath", { status: "ok", data: "/库/甲/图.png" });
    const widget = new ImageEmbedWidget("甲.md", "图-加载失败.png", "说明", null, undefined, 9);
    const fakeView = { dom: document.createElement("div") } as never;
    const dom = widget.toDOM(fakeView);
    dom.querySelector("img")!.dispatchEvent(new Event("error"));
    expect(dom.dataset.broken).toBe("true");
    expect(dom.textContent).toBe("说明");
  });
});
