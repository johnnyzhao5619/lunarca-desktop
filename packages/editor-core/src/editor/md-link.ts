/**
 * 编辑面里**标准 Markdown 链接**（`[文本](目标)`）的点击（PRD-06-030）。
 *
 * 阅读面早就能点了，编辑面此前只认 `[[wiki]]`——而**全部自动生成的溯源链接都是标准相对
 * md 链接**（蒸馏 / 汇总 / 主题页初稿的「来源」区逐条如此）。于是同一篇产物，在阅读模式
 * 里来源点得动，切回编辑模式就点不动了（真机 2026-09-09）。同一个对象在两个模式下能做的
 * 事不该分叉。
 *
 * **只在折叠态生效**：抓手是 live preview 挂的 `.luna-md-linkbody`，它只在链接被折叠
 * 成一段文字时存在。语法高亮那份 `.luna-md-link` 在源码模式下同样在场，按它判会让"点一下
 * 想改 URL"变成"跳走了"。
 *
 * **挂在 `mousedown` 而不是 `click`**（真机 2026-09-09 第二轮）：CodeMirror 在
 * `mousedown` 时就把光标移进来了，链接**当场展开成源码**，那个只在折叠态存在的抓手随之
 * 从 DOM 上消失——等到 `click` 触发时 `closest()` 已经找不到它，于是"编辑模式下点来源
 * 毫无反应"。`mousedown` 那一刻 DOM 还是折叠态。顺带 `preventDefault()` 掐掉光标移动：
 * 点一条折叠着的链接，意图是过去，不是把光标插进它中间。
 *
 * 与 wiki-link 的差别正在这里：`wikiLinkExtension` 的 `.luna-wiki-link` **不分折叠与否
 * 恒在**，故它挂 `click` 也找得到。本条的抓手是折叠态专有的，时机就必须往前挪。
 *
 * 外链与库内链接都交宿主（`EditorHost.markdown` / `EditorHost.vault`），与阅读面同一个端口——`../` 归一、
 * 越界判定与"存不存在"是文件系统的事实（桌面问 Rust，ADR-003），两个模式各算一遍就会有一头对不上。
 */

import { EditorView } from "@codemirror/view";
import { syntaxTree } from "@codemirror/language";
import type { EditorHost } from "./editor-host";

export interface MdLinkContext {
  /** 当前笔记的库内相对路径（相对链接的解析基准）；`null` = 还没有落盘的笔记。 */
  notePath: () => string | null;
  onOpenPath: (relPath: string) => void;
}

/** 光标位置所在的那条 Markdown 链接的目标（没有就是 `null`）。 */
function linkTargetAt(view: EditorView, pos: number): string | null {
  let node = syntaxTree(view.state).resolveInner(pos, 1);
  while (node.name !== "Link") {
    const parent = node.parent;
    if (!parent) {
      return null;
    }
    node = parent;
  }
  const cursor = node.cursor();
  if (!cursor.firstChild()) {
    return null;
  }
  do {
    if (cursor.name === "URL") {
      return view.state.sliceDoc(cursor.from, cursor.to).trim();
    }
  } while (cursor.nextSibling());
  return null;
}

/** 编辑面里折叠成文字的那条 md 链接（`.luna-md-linkbody`）的目标；不是就 `null`。 */
export function mdLinkTargetAtElement(view: EditorView, element: Element): string | null {
  const body = element.closest(".luna-md-linkbody");
  if (!body || !view.contentDOM.contains(body)) return null;
  const pos = view.posAtDOM(body);
  return pos < 0 ? null : linkTargetAt(view, pos);
}

export function mdLinkClickHandler(host: EditorHost, ctx: MdLinkContext) {
  return EditorView.domEventHandlers({
    mousedown: (event, view) => {
      // 只接**朴素左键**：带修饰键的那些是 CodeMirror 自己的（多光标、块选），
      // 右键要留给上下文菜单。
      if (event.button !== 0 || event.metaKey || event.ctrlKey || event.altKey || event.shiftKey) {
        return false;
      }
      if (!(event.target instanceof Element)) {
        return false;
      }
      const href = mdLinkTargetAtElement(view, event.target);
      if (!href) {
        return false;
      }
      event.preventDefault();
      if (host.markdown.isExternalUrlAllowed(href)) {
        host.markdown.openExternalUrl(href);
        return true;
      }
      const from = ctx.notePath();
      if (from === null) {
        return true;
      }
      // 指不到就什么都不做——不猜一个"大概是这篇"（宿主的约定，与阅读面同一个端口）。
      host.vault.openVaultLink(from, href, ctx.onOpenPath);
      return true;
    },
  });
}
