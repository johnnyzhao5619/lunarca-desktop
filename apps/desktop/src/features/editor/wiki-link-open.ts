// 点一条 wiki-link 之后的事（编辑器与阅读视图共用一份）：问 Rust 指向哪一篇、落在哪一行，
// 打开并定位；库里没有就按 Rust 给的路径新建再打开。前端不比对文件名、不拼路径（ADR-003）。

import { commands } from "../../ipc/bindings";
import { requestEditorReveal } from "../../lib/app-events";

export interface OpenWikiLinkDeps {
  onOpenPath: (path: string) => void;
  /** 指不到时可否新建（只读的阅读面不建）。 */
  allowCreate: boolean;
  onError: () => void;
}

/** 编辑器 / 阅读视图着色用的批量解析；失败时返回 `null`（保持原样，不把链接全标成未创建）。 */
export async function resolveWikiTargets(
  notePath: string,
  targets: string[],
): Promise<(string | null)[] | null> {
  const res = await commands.vaultResolveWikilinks(notePath, targets);
  return res.status === "ok" ? res.data : null;
}

export async function openWikiLink(notePath: string, target: string, deps: OpenWikiLinkDeps): Promise<void> {
  const res = await commands.vaultResolveWikilink(notePath, target);
  if (res.status !== "ok") {
    deps.onError();
    return;
  }
  const hit = res.data;
  if (!hit.exists) {
    if (!deps.allowCreate) return;
    const created = await commands.vaultCreate(hit.path, "note", "");
    if (created.status !== "ok") {
      deps.onError();
      return;
    }
    deps.onOpenPath(created.data.path);
    return;
  }
  deps.onOpenPath(hit.path);
  if (hit.line !== null) {
    requestEditorReveal({ path: hit.path, line: hit.line, headingIndex: hit.heading_index, focusEditor: true });
  }
}
