/**
 * 图表的明暗，与样式 Token 的取色同一个判据（`themes.generated.css`）：根节点设了 `data-theme` 就听它
 * （桌面），没设就跟随系统的 `prefers-color-scheme`（网页）。两者变了都会重渲。
 */

import { useSyncExternalStore } from "react";
import type { DiagramTheme } from "../host";

const DARK_QUERY = "(prefers-color-scheme: dark)";

function systemPrefersDark(): boolean {
  return typeof window.matchMedia === "function" && window.matchMedia(DARK_QUERY).matches;
}

export function diagramTheme(): DiagramTheme {
  const explicit = document.documentElement.dataset.theme;
  if (explicit !== undefined) return explicit === "dark" ? "dark" : "default";
  return systemPrefersDark() ? "dark" : "default";
}

function subscribe(onChange: () => void): () => void {
  const media = typeof window.matchMedia === "function" ? window.matchMedia(DARK_QUERY) : null;
  media?.addEventListener("change", onChange);
  const observer = new MutationObserver(onChange);
  observer.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
  return () => {
    media?.removeEventListener("change", onChange);
    observer.disconnect();
  };
}

export function useDiagramTheme(): DiagramTheme {
  return useSyncExternalStore(subscribe, diagramTheme, () => "default");
}
