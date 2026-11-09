/**
 * 桌面对共享编辑器（`@lunarca/editor-core` 的 CM6 扩展）的宿主端口实现（TDD-17 §2）。每个编辑器都传这
 * 一份；判定全在 Rust（ADR-003），这里只把编辑器的问题转成 IPC：
 * - 渲染与库级能力直接用阅读面那两份（`desktopMarkdownHost` / `desktopReadingVault`），编辑面与阅读面
 *   在桌面上拿到的是同一份实现；
 * - 附件变化来自文件监听（`media/asset-changes.ts`）；
 * - `[[` / `#` 补全候选问 Rust（`vault_wikilink_candidates` / `tag_summaries`）；
 * - 属性面板的规则问 Rust（`note_properties` / `note_property_edit`，同一份 `luna_sync_core::frontmatter_edit`
 *   网页经 WASM 调）；面板与格式工具条里的按钮、下拉与菜单用桌面的控件原语（`editor-ui.tsx`）；
 * - 「Tab 键移动焦点」是桌面会话级的那一份开关（`lib/tab-focus-mode.ts`，状态栏读它）。
 */

import type { EditorHost, PropertyRules } from "@lunarca/editor-core/editor-host";
import { desktopMarkdownHost } from "../../components/markdown/markdown-host";
import { commands } from "../../ipc/bindings";
import { toggleTabFocusMode } from "../../lib/tab-focus-mode";
import { currentAssetGeneration, onAssetsChanged } from "../media/asset-changes";
import { desktopEditorUi } from "./editor-ui";
import { desktopReadingVault } from "./ReadingView";

const desktopPropertyRules: PropertyRules = {
  read: async (source) => {
    const result = await commands.noteProperties(source);
    return result.status === "ok" ? { ok: true, value: result.data } : { ok: false, skip: result.error };
  },
  edit: async (source, edit) => {
    const result = await commands.notePropertyEdit(source, edit);
    return result.status === "ok" ? { ok: true, value: result.data } : { ok: false, skip: result.error };
  },
};

export const desktopEditorHost: EditorHost = {
  markdown: desktopMarkdownHost,
  vault: desktopReadingVault,
  assets: { subscribe: onAssetsChanged, generation: currentAssetGeneration },
  wikiLinkCandidates: async (notePath, query) => {
    const result = await commands.vaultWikilinkCandidates(notePath, query);
    return result.status === "ok" ? result.data : [];
  },
  tagCounts: async () => {
    const result = await commands.tagSummaries();
    return result.status === "ok" ? result.data.tags.map((tag) => ({ tag: tag.tag, noteCount: tag.note_count })) : null;
  },
  properties: desktopPropertyRules,
  ui: desktopEditorUi,
  toggleTabFocusMode,
};
