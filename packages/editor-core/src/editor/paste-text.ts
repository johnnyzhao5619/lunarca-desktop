// 粘贴与插入附件链接共用的纯逻辑（PRD-06-012）：剪贴板文本是不是单个网址 / 图片网址、库内附件该写成
// `![](…)` 还是 `[名](…)`、按光标插入纯文本。附件怎么落盘、HTML 怎么转 Markdown 都归宿主（桌面的
// `paste-assets.ts` 问 Rust），这里不碰。

import { EditorSelection } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";

/** 剪贴板文本是否为单个 http(s) URL（无内嵌空白）——用于「选区包成链接」判定。 */
export function isSingleUrl(text: string): boolean {
  const trimmed = text.trim();
  return /^https?:\/\/\S+$/u.test(trimmed);
}

const IMAGE_URL_EXT_RE = /\.(?:png|jpe?g|gif|webp|svg|bmp|tiff?|avif|ico)(?:[?#].*)?$/iu;

/** 单个 http(s) URL 且路径以图片扩展名结尾 → 视为可下载本地化的远程图片链接。 */
export function isImageUrl(text: string): boolean {
  return isSingleUrl(text) && IMAGE_URL_EXT_RE.test(text.trim());
}

/** 库内附件相对路径是否为图片（决定回填 `![](…)` 还是 `[名](…)`；与远程图片链接同一扩展名单）。 */
export function isImageAssetPath(relPath: string): boolean {
  return IMAGE_URL_EXT_RE.test(relPath);
}

/** 挑出剪贴板中的文件项（图片/附件），排除纯文本项。 */
export function pickFileItems(items: DataTransferItemList): DataTransferItem[] {
  return Array.from(items).filter((item) => item.kind === "file");
}

/** 附件相对路径 + 是否图片 → 插入用的 Markdown（图片 `![](p)`；其它 `[名](p)`）。 */
export function assetMarkdown(relPath: string, isImage: boolean): string {
  if (isImage) {
    return `![](${relPath})`;
  }
  const name = relPath.split("/").pop() ?? relPath;
  return `[${name}](${relPath})`;
}

/**
 * 纯文本插入到每个光标（多光标 B5）：行数恰好等于光标数时每个光标分一行（VS Code / CodeMirror
 * 自带粘贴同一规则——复制了三行、三个光标，就各得一行），否则每处都插整段。
 */
export function insertAtCursor(view: EditorView, text: string) {
  const { state } = view;
  const lines = state.toText(text);
  const perRange = state.selection.ranges.length > 1 && lines.lines === state.selection.ranges.length;
  let index = 0;
  const transaction = state.changeByRange((range) => {
    const insert = perRange ? lines.line(++index).text : text;
    return {
      changes: { from: range.from, to: range.to, insert },
      range: EditorSelection.cursor(range.from + state.toText(insert).length),
    };
  });
  view.dispatch(transaction, { userEvent: "input.paste", scrollIntoView: true });
}
