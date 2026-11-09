/**
 * 图表渲染沙箱页（`/render-sandbox.html`，TDD-17 §6.1，PRD-21-012）的脚本。这一页装在主页面的
 * `<iframe sandbox="allow-scripts">` 里，是不透明来源；它有自己的一份 CSP：不开 Trusted Types、样式放行
 * `'unsafe-inline'`（Mermaid 往 SVG 里写 `<style>` 与 `style` 属性）、一个请求都不许发。
 *
 * 只收 `window.parent` 发来、形状对的请求，用 Mermaid（`securityLevel: "strict"`，`htmlLabels: false`：
 * 标签是 SVG 文本而不是 foreignObject）画成 SVG，原样回给父页面；一次只画一张。
 */

import mermaid from "mermaid";
import { parseRenderRequest, type RenderReply, type RenderRequest } from "./protocol";

let configured: string | null = null;
let sequence = 0;
let queue: Promise<void> = Promise.resolve();

function configure(theme: RenderRequest["theme"], font: string) {
  const key = `${theme}\n${font}`;
  if (configured === key) return;
  mermaid.initialize({
    startOnLoad: false,
    securityLevel: "strict",
    theme,
    fontFamily: font,
    htmlLabels: false,
    flowchart: { htmlLabels: false },
    suppressErrorRendering: true,
    logLevel: "fatal",
  });
  configured = key;
}

/**
 * 以图片显示的 SVG 要是独立的 XML 文档：Mermaid 给的是 HTML 序列化（会有 `&nbsp;` 一类 XML 不认的写法），
 * 这里按 XML 重新序列化；宽高钉成 viewBox 的尺寸（Mermaid 写的是 `width="100%"` 加 `max-width`），
 * 图片才有固有尺寸，缩放交给主页面的 CSS。
 */
function asStandaloneSvg(markup: string): string {
  const svg = new DOMParser().parseFromString(markup, "text/html").querySelector("svg");
  if (svg === null) throw new Error("Mermaid returned no SVG");
  const box = svg.viewBox.baseVal;
  if (box !== null && box.width > 0 && box.height > 0) {
    svg.setAttribute("width", String(box.width));
    svg.setAttribute("height", String(box.height));
  }
  svg.style.removeProperty("max-width");
  return new XMLSerializer().serializeToString(svg);
}

async function draw(request: RenderRequest): Promise<RenderReply> {
  try {
    configure(request.theme, request.font);
    const { svg } = await mermaid.render(`luna-diagram-${++sequence}`, request.source);
    return { id: request.id, svg: asStandaloneSvg(svg) };
  } catch (error) {
    return { id: request.id, error: error instanceof Error ? error.message : String(error) };
  }
}

window.addEventListener("message", (event) => {
  // 父页面与本页的地址同源（不透明的只是本页的来源，`location.origin` 仍是地址的来源）。
  if (event.source !== window.parent || event.source === window || event.origin !== location.origin) return;
  const request = parseRenderRequest(event.data);
  if (request === null) return;
  queue = queue.then(async () => {
    window.parent.postMessage(await draw(request), location.origin);
  });
});
