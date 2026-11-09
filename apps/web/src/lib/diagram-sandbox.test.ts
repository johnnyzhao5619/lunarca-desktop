// @vitest-environment happy-dom
/**
 * 网页图表端口（PRD-21-012、TDD-17 §6.1）：只认沙箱框架发来、来源为 "null"、形状对的回信；图片地址交还即撤销；
 * 沙箱报错或没回信都回落成 `error`（渲染器据此显示源码与原因）。
 */

import { describe, expect, it, vi } from "vitest";
import type { RenderRequest } from "../sandbox/protocol";
import { acceptReply, createDiagramSandbox } from "./diagram-sandbox";

/** 沙箱框架的替身：`contentWindow` 记下发过去的请求，`load()` 模拟页面载完。 */
function fakeFrame() {
  const posted: Array<{ message: RenderRequest; targetOrigin: string }> = [];
  const contentWindow = {
    postMessage: (message: RenderRequest, targetOrigin: string) => posted.push({ message, targetOrigin }),
  } as unknown as Window;
  const loads: Array<() => void> = [];
  const frame = {
    contentWindow,
    addEventListener: (type: string, listener: () => void) => {
      if (type === "load") loads.push(listener);
    },
  } as unknown as HTMLIFrameElement;
  return { frame, contentWindow, posted, load: () => loads.forEach((listener) => listener()) };
}

function harness(timeoutMs = 1_000) {
  const frame = fakeFrame();
  const host = new EventTarget();
  const createFrame = vi.fn(() => frame.frame);
  const blobs: Blob[] = [];
  const revoked: string[] = [];
  const diagrams = createDiagramSandbox({
    createFrame,
    host: host as unknown as Window,
    createObjectURL: (blob) => {
      blobs.push(blob);
      return `blob:test/${blobs.length}`;
    },
    revokeObjectURL: (url) => revoked.push(url),
    font: () => "serif",
    timeoutMessage: () => "timed out",
    timeoutMs,
  });
  /** 以给定的来源与发送方往主页面投一封信。 */
  function deliver(data: unknown, { origin = "null", source = frame.contentWindow as unknown }: { origin?: string; source?: unknown } = {}) {
    host.dispatchEvent(new MessageEvent("message", { data, origin, source: source as Window }));
  }
  return { ...frame, createFrame, blobs, revoked, diagrams, deliver };
}

/** 等请求真的发出去（框架载完之后才发）。 */
async function flush() {
  await new Promise((resolve) => setTimeout(resolve, 0));
}

describe("acceptReply：主页面的回信校验", () => {
  const frame = {} as Window;
  const event = (data: unknown, origin = "null", source: unknown = frame) =>
    new MessageEvent("message", { data, origin, source: source as Window });

  it("来自沙箱框架、来源为 null、形状对的才收", () => {
    expect(acceptReply(event({ id: 1, svg: "<svg/>" }), frame)).toEqual({ id: 1, svg: "<svg/>" });
    expect(acceptReply(event({ id: 2, error: "bad" }), frame)).toEqual({ id: 2, error: "bad" });
  });

  it("来源不对的一律丢弃：别的窗口、同源页面、还没有框架", () => {
    expect(acceptReply(event({ id: 1, svg: "<svg/>" }, "null", {} as Window), frame)).toBeNull();
    expect(acceptReply(event({ id: 1, svg: "<svg/>" }, "null", null), frame)).toBeNull();
    expect(acceptReply(event({ id: 1, svg: "<svg/>" }, "http://127.0.0.1:4183"), frame)).toBeNull();
    expect(acceptReply(event({ id: 1, svg: "<svg/>" }, "https://evil.test"), frame)).toBeNull();
    expect(acceptReply(event({ id: 1, svg: "<svg/>" }), null)).toBeNull();
  });

  it("形状不对的一律丢弃", () => {
    for (const data of [
      null,
      "svg",
      [1, "<svg/>"],
      { id: "1", svg: "<svg/>" },
      { id: 0, svg: "<svg/>" },
      { id: 1.5, svg: "<svg/>" },
      { id: 1 },
      { id: 1, svg: 42 },
      { id: 1, error: { message: "x" } },
      { id: 1, svg: "<svg/>", error: "x" },
      { id: 1, svg: "<svg/>", extra: true },
    ]) {
      expect(acceptReply(event(data), frame), JSON.stringify(data)).toBeNull();
    }
  });
});

describe("createDiagramSandbox", () => {
  it("第一次画图才建框架，载完才发请求；回信做成 Blob URL 的图片，交还即撤销且只撤一次", async () => {
    const h = harness();
    expect(h.createFrame).not.toHaveBeenCalled();
    const pending = h.diagrams.render("graph TD\n  A --> B", "dark");
    expect(h.createFrame).toHaveBeenCalledTimes(1);
    await flush();
    expect(h.posted).toEqual([]);
    h.load();
    await flush();
    expect(h.posted).toEqual([{ message: { id: 1, source: "graph TD\n  A --> B", theme: "dark", font: "serif" }, targetOrigin: "*" }]);
    h.deliver({ id: 1, svg: "<svg/>" });
    const result = await pending;
    expect(result).toMatchObject({ kind: "image", url: "blob:test/1" });
    expect(h.blobs[0].type).toBe("image/svg+xml");
    if (result.kind !== "image") throw new Error("unreachable");
    expect(h.revoked).toEqual([]);
    result.release();
    result.release();
    expect(h.revoked).toEqual(["blob:test/1"]);
  });

  it("冒充的回信不算数：来源不对、发送方不对的被丢掉，真回信照样配对", async () => {
    const h = harness();
    const pending = h.diagrams.render("graph TD", "default");
    h.load();
    await flush();
    h.deliver({ id: 1, svg: "<svg id='forged'/>" }, { origin: "http://127.0.0.1:4183" });
    h.deliver({ id: 1, svg: "<svg id='forged'/>" }, { source: {} });
    h.deliver({ id: 1, svg: "<svg id='forged'/>", extra: 1 });
    h.deliver({ id: 1, svg: "<svg id='real'/>" });
    await expect(pending).resolves.toMatchObject({ kind: "image" });
    expect(h.blobs).toHaveLength(1);
    expect(await h.blobs[0].text()).toBe("<svg id='real'/>");
  });

  it("同主题同字体同源码走缓存：不再发请求，但每次给新的地址（各自交还）", async () => {
    const h = harness();
    const first = h.diagrams.render("graph TD", "default");
    h.load();
    await flush();
    h.deliver({ id: 1, svg: "<svg/>" });
    const a = await first;
    const b = await h.diagrams.render("graph TD", "default");
    expect(h.posted).toHaveLength(1);
    expect(a).toMatchObject({ url: "blob:test/1" });
    expect(b).toMatchObject({ url: "blob:test/2" });
    void h.diagrams.render("graph TD", "dark");
    await flush();
    expect(h.posted).toHaveLength(2);
  });

  it("沙箱报错：回落成 error，带 Mermaid 的说明；不缓存", async () => {
    const h = harness();
    const pending = h.diagrams.render("graph TD\n  A -->", "default");
    h.load();
    await flush();
    h.deliver({ id: 1, error: "Parse error on line 2" });
    await expect(pending).resolves.toEqual({ kind: "error", message: "Parse error on line 2" });
    void h.diagrams.render("graph TD\n  A -->", "default");
    await flush();
    expect(h.posted).toHaveLength(2);
  });

  it("沙箱没在时限内回信（含框架没载完）：回落成 error，迟到的回信不再生效", async () => {
    vi.useFakeTimers();
    try {
      const h = harness(50);
      const neverLoaded = h.diagrams.render("graph TD", "default");
      await vi.advanceTimersByTimeAsync(60);
      await expect(neverLoaded).resolves.toEqual({ kind: "error", message: "timed out" });
      h.load();
      const late = h.diagrams.render("graph LR", "default");
      await vi.advanceTimersByTimeAsync(60);
      await expect(late).resolves.toEqual({ kind: "error", message: "timed out" });
      h.deliver({ id: 2, svg: "<svg/>" });
      expect(h.revoked).toEqual([]);
    } finally {
      vi.useRealTimers();
    }
  });
});
