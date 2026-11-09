/**
 * 网页对共享编辑器的宿主端口（TDD-17 §2、PRD-21-020）：
 * - 渲染与库级能力与阅读面同一份（`webMarkdownHost` / `webReadingVault`，判定在 Worker 的 WASM 里）；
 * - `[[` 补全候选来自 Worker 的清单视图（排序与同名消歧与桌面同一份）；
 * - `#` 补全：网页只有清单、拿不到全部正文，给 `null`——补全里只是不提供标签；
 * - 属性面板与桌面同一个组件（`@lunarca/editor-core`），规则问 Worker 里的 WASM（`note_properties` /
 *   `note_property_edit`，与桌面同一份 `luna_sync_core::frontmatter_edit`，改出来的字节一致）；按钮、下拉与菜单
 *   用网页的控件原语（`web-editor-ui.tsx`）；
 * - 图片的 Blob URL：编辑面经这里取的都记下来，换笔记或卸载时 `releaseImages()` 一次交还，并推进附件
 *   代际号——编辑器的图片缓存随之作废、还开着的编辑器按新地址重新取；
 * - 「Tab 键移动焦点」是本页会话级的一份开关；
 * - 拖入与粘贴的附件经 Worker 上传后插入链接（`features/media/upload-extension.ts`），进展报给工作区。
 */

import type { Extension } from "@codemirror/state";
import type { EditorHost, PropertyResult, PropertyRules, PropertySkip } from "@lunarca/editor-core/editor-host";
import { webMarkdownHost } from "../../lib/markdown-host";
import { WorkerError, type WorkerClient } from "../../worker/worker-client";
import { UploadReporter, uploadExtension, uploadPicked, type UploadTarget } from "../media/upload-extension";
import { webReadingVault } from "../reader/web-reading-vault";
import { toggleTabFocusMode } from "./tab-focus-mode";
import { webEditorUi } from "./web-editor-ui";

export interface WebEditorHost extends EditorHost {
  /** 交还编辑面取过的全部图片地址（换笔记、卸载）。 */
  releaseImages(): void;
  /** 拖入与粘贴上传的进展（工作区据此写明正在上传、为什么没传上）。 */
  uploads: UploadReporter;
  /** 编辑器的拖入与粘贴上传（放进 `afterCompletions` 插槽）。 */
  uploadExtension(target: UploadTarget): Extension;
  /** 「插入附件」选中的文件：上传后返回链接（与拖入同一条路）。 */
  uploadFiles(target: UploadTarget, files: readonly File[]): Promise<string[]>;
}

/** 属性面板能如实说明的原因码（Rust `EditSkip` 的稳定码）。 */
const PROPERTY_SKIPS: ReadonlySet<string> = new Set<PropertySkip>([
  "unsupported_form",
  "inline_comment",
  "multi_line",
  "nested",
  "duplicate_key",
  "unclosed",
  "key_exists",
  "invalid_key",
  "not_found",
  "invalid_value",
]);

/** 原因码照实回给面板；别的失败（Worker 起不来之类）抛出去，面板保持原状。 */
async function propertyResult<T>(call: Promise<T>): Promise<PropertyResult<T>> {
  try {
    return { ok: true, value: await call };
  } catch (error) {
    if (error instanceof WorkerError && PROPERTY_SKIPS.has(error.code)) return { ok: false, skip: error.code as PropertySkip };
    throw error;
  }
}

function webPropertyRules(client: WorkerClient): PropertyRules {
  return {
    read: (source) => propertyResult(client.call("note_properties", { source })),
    edit: (source, edit) => propertyResult(client.call("note_property_edit", { source, edit })),
  };
}

export function webEditorHost(client: WorkerClient): WebEditorHost {
  const reading = webReadingVault(client);
  const releases = new Set<() => void>();
  const listeners = new Set<() => void>();
  let generation = 0;
  const uploads = new UploadReporter();
  return {
    markdown: webMarkdownHost,
    vault: {
      ...reading,
      imageSource: async (notePath, src) => {
        const image = await reading.imageSource(notePath, src);
        if (image.release) releases.add(image.release);
        return image;
      },
    },
    assets: {
      subscribe: (listener) => {
        listeners.add(listener);
        return () => {
          listeners.delete(listener);
        };
      },
      generation: () => generation,
    },
    wikiLinkCandidates: (notePath, query) =>
      client.call("wiki_candidates", { note_path: notePath, query }).catch(() => []),
    // `#` 补全只用已经建好的全文索引：补全本身不该触发下载全部正文（PRD-21-041 由用户发起）。
    tagCounts: async () => {
      try {
        if ((await client.call("fulltext_state", {})).state !== "ready") return null;
        const tags = await client.call("tags", {});
        return tags.map((entry) => ({ tag: entry.tag, noteCount: entry.count }));
      } catch {
        return null;
      }
    },
    properties: webPropertyRules(client),
    ui: webEditorUi,
    toggleTabFocusMode,
    releaseImages: () => {
      if (releases.size === 0) return;
      for (const release of releases) release();
      releases.clear();
      generation += 1;
      for (const listener of listeners) listener();
    },
    uploads,
    uploadExtension: (target) => uploadExtension({ call: (cmd, args) => client.call(cmd, args), report: uploads.report }, target),
    uploadFiles: (target, files) => uploadPicked({ call: (cmd, args) => client.call(cmd, args), report: uploads.report }, target, files),
  };
}
