/**
 * 图表渲染沙箱的消息协议（TDD-17 §6.1，PRD-21-012）。两边各自校验来源与形状，形状不对的消息一律丢弃：
 * - 主页面 → 沙箱：`{ id, source, theme, font }`——`font` 是阅读面正文的字体栈，图里的字与正文同一套；
 * - 沙箱 → 主页面：`{ id, svg }` 或 `{ id, error }`。
 * 沙箱页是不透明来源（`sandbox="allow-scripts"`、不给 `allow-same-origin`），它发来的消息 `origin` 是 `"null"`。
 */

import type { DiagramTheme } from "@lunarca/editor-core/host";

export interface RenderRequest {
  id: number;
  source: string;
  theme: DiagramTheme;
  font: string;
}

export type RenderReply = { id: number; svg: string } | { id: number; error: string };

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function hasExactKeys(value: Record<string, unknown>, keys: readonly string[]): boolean {
  const own = Object.keys(value);
  return own.length === keys.length && keys.every((key) => Object.hasOwn(value, key));
}

function isRequestId(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value > 0;
}

export function parseRenderRequest(data: unknown): RenderRequest | null {
  if (!isRecord(data) || !hasExactKeys(data, ["id", "source", "theme", "font"])) return null;
  const { id, source, theme, font } = data;
  if (!isRequestId(id) || typeof source !== "string" || typeof font !== "string") return null;
  if (theme !== "default" && theme !== "dark") return null;
  return { id, source, theme, font };
}

export function parseRenderReply(data: unknown): RenderReply | null {
  if (!isRecord(data) || !isRequestId(data.id)) return null;
  if (hasExactKeys(data, ["id", "svg"]) && typeof data.svg === "string") return { id: data.id, svg: data.svg };
  if (hasExactKeys(data, ["id", "error"]) && typeof data.error === "string") return { id: data.id, error: data.error };
  return null;
}
