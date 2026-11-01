/**
 * 创作空间主区此刻在哪（ADR-165 ⑥）——一份状态，而不是三个各管一页的开关。
 *
 * 之前主区是「发布包页 / 概览页 / 灵感库页 / 看板」四个布尔 state 按优先级叠出来的，哪一页
 * 在上面要看渲染顺序；现在只有一个值，深链、侧栏、看板与编辑器里的链接都改这一个值。
 */

import type { CreatorTarget } from "../workspace/workspace-routes";
import type { ProjectPane } from "./ProjectWorkspace";

export type CreatorView =
  | { view: "board" }
  | { view: "swipes" }
  | { view: "project"; home: string; pane: ProjectPane }
  /** 一篇不属于某个项目的创作笔记（灵感条目）：在创作空间里编辑，不跳去「笔记」。 */
  | { view: "note"; path: string };

/** 深链落点 → 主区状态。 */
export function creatorViewOf(target: CreatorTarget): CreatorView {
  switch (target.view) {
    case "board":
      return { view: "board" };
    case "swipes":
      return { view: "swipes" };
    case "project":
      return { view: "project", home: target.home, pane: { tab: target.tab } };
  }
}

/** 笔记改名或移动之后，主区里指着旧路径的地方跟着改。 */
export function remapCreatorView(view: CreatorView, from: string, to: string): CreatorView {
  switch (view.view) {
    case "project": {
      const home = view.home === from ? to : view.home;
      const pane = view.pane.tab === "doc" && view.pane.path === from ? { tab: "doc" as const, path: to } : view.pane;
      return home === view.home && pane === view.pane ? view : { view: "project", home, pane };
    }
    case "note":
      return view.path === from ? { view: "note", path: to } : view;
    default:
      return view;
  }
}

/** 侧栏里该高亮哪一项（项目看主页，灵感看它自己）。 */
export function creatorActivePath(view: CreatorView): string | null {
  switch (view.view) {
    case "project":
      return view.home;
    case "note":
      return view.path;
    default:
      return null;
  }
}
