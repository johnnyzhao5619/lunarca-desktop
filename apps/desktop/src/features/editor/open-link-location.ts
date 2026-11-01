/**
 * 点了一条库内标准 md 链接之后：打开那篇，再按 Rust 解析出的片段到那一行、从那一刻播放
 * （ADR-162 ⑭：引文出处 `#t=` / `#L`）。编辑面与阅读面共用这一处，免得两个模式各做一半。
 */

import type { NoteLinkLocationDto } from "../../ipc/bindings";
import { requestEditorReveal, requestPlaybackAt } from "../../lib/app-events";

export function openLinkLocation(location: NoteLinkLocationDto, onOpenPath: (path: string) => void): void {
  onOpenPath(location.path);
  if (location.line !== null) {
    requestEditorReveal({ path: location.path, line: location.line, headingIndex: null, focusEditor: true });
  }
  if (location.seek_ms !== null) {
    requestPlaybackAt({ path: location.path, ms: location.seek_ms });
  }
}
