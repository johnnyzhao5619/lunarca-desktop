/**
 * 网页的图表端口（`MarkdownHost.diagrams` 的 `image` 形态，PRD-21-012、TDD-17 §6.1）：Mermaid 在主页面的
 * CSP 下画不了，交给同源的沙箱页 `/render-sandbox.html` 画。
 * - 沙箱页装在 `<iframe sandbox="allow-scripts">` 里、不给 `allow-same-origin`：它是不透明来源，拿不到
 *   Cookie 与存储、碰不到主页面，CSP 又让它一个请求都发不出去；
 * - 主页面只发图表源码过去，回来的 SVG 做成 `image/svg+xml` 的 Blob URL，用 `<img>` 显示——以图片显示的
 *   SVG 不执行脚本、不加载外部资源；SVG 一律不内联进主页面的 DOM；
 * - 只认来自这个框架的 `contentWindow`、`origin === "null"`、形状对的回信，其余丢弃。
 * 框架在第一次要画图时创建，之后复用到底（隐藏，但照常排版：Mermaid 要量字宽）。
 */

import type { DiagramImage, DiagramTheme, ImageDiagrams } from "@lunarca/editor-core/host";
import { parseRenderReply, type RenderReply, type RenderRequest } from "../sandbox/protocol";

export const SANDBOX_PATH = "/render-sandbox.html";
export const SANDBOX_CLASS = "luna-web-render-sandbox";
const TIMEOUT_MS = 30_000;
const CACHE_LIMIT = 32;

/** 沙箱的回信：来源必须是沙箱框架本身、`origin` 必须是不透明来源的 `"null"`，形状必须对。 */
export function acceptReply(event: MessageEvent, frame: Window | null): RenderReply | null {
  if (frame === null || event.source !== frame || event.origin !== "null") return null;
  return parseRenderReply(event.data);
}

export interface DiagramSandboxDeps {
  /** 建好并挂上沙箱框架（`src` 已指向沙箱页）。 */
  createFrame(): HTMLIFrameElement;
  /** 收消息的窗口（主页面的 `window`）。 */
  host: Pick<Window, "addEventListener">;
  createObjectURL(blob: Blob): string;
  revokeObjectURL(url: string): void;
  /** 阅读面正文的字体栈：图里的字与正文同一套。 */
  font(): string;
  /** 沙箱没在时限内回信时给人看的原因。 */
  timeoutMessage(): string;
  timeoutMs?: number;
}

export function createDiagramSandbox(deps: DiagramSandboxDeps): ImageDiagrams {
  const timeoutMs = deps.timeoutMs ?? TIMEOUT_MS;
  const pending = new Map<number, (reply: RenderReply | null) => void>();
  /** 「主题 + 字体 + 源码」→ SVG：换笔记再回来不必重画；每次照样给一个新的 Blob URL。 */
  const cache = new Map<string, string>();
  let frame: HTMLIFrameElement | null = null;
  let ready: Promise<Window | null> | null = null;
  let nextId = 0;

  function onMessage(event: MessageEvent) {
    const reply = acceptReply(event, frame?.contentWindow ?? null);
    if (reply === null) return;
    const settle = pending.get(reply.id);
    if (settle === undefined) return;
    pending.delete(reply.id);
    settle(reply);
  }

  function frameWindow(): Promise<Window | null> {
    ready ??= new Promise((resolve) => {
      deps.host.addEventListener("message", onMessage);
      const created = deps.createFrame();
      frame = created;
      created.addEventListener("load", () => resolve(created.contentWindow), { once: true });
    });
    return ready;
  }

  function request(message: Omit<RenderRequest, "id">): Promise<RenderReply | null> {
    const id = ++nextId;
    return new Promise((resolve) => {
      const timer = setTimeout(() => {
        pending.delete(id);
        resolve(null);
      }, timeoutMs);
      pending.set(id, (reply) => {
        clearTimeout(timer);
        resolve(reply);
      });
      void frameWindow().then((target) => {
        // 不透明来源没有可写的目标 origin，只能用 "*"；收信方就是上面这个框架。
        target?.postMessage({ id, ...message } satisfies RenderRequest, "*");
      });
    });
  }

  function image(svg: string): DiagramImage {
    const url = deps.createObjectURL(new Blob([svg], { type: "image/svg+xml" }));
    let released = false;
    return {
      kind: "image",
      url,
      release: () => {
        if (released) return;
        released = true;
        deps.revokeObjectURL(url);
      },
    };
  }

  function remember(key: string, svg: string) {
    cache.delete(key);
    cache.set(key, svg);
    if (cache.size > CACHE_LIMIT) cache.delete(cache.keys().next().value as string);
  }

  return {
    kind: "image",
    async render(source: string, theme: DiagramTheme): Promise<DiagramImage> {
      const font = deps.font();
      const key = `${theme}\n${font}\n${source}`;
      const hit = cache.get(key);
      if (hit !== undefined) return image(hit);
      const reply = await request({ source, theme, font });
      if (reply === null) return { kind: "error", message: deps.timeoutMessage() };
      if ("error" in reply) return { kind: "error", message: reply.error };
      remember(key, reply.svg);
      return image(reply.svg);
    },
  };
}

/** 浏览器里的那一份：隐藏框架挂在 `<body>` 末尾。 */
export function createSandboxFrame(): HTMLIFrameElement {
  const frame = document.createElement("iframe");
  frame.setAttribute("sandbox", "allow-scripts");
  frame.className = SANDBOX_CLASS;
  frame.setAttribute("aria-hidden", "true");
  frame.tabIndex = -1;
  frame.src = SANDBOX_PATH;
  document.body.append(frame);
  return frame;
}
