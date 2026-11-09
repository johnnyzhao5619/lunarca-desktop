/**
 * 深链 → **落点**（ADR-087 ② / ADR-088 / PRD-15-040）：一份纯函数解析器。
 *
 * 深链是"从通知、编辑器、图谱、标签、队列、启动概览到具体目标"的唯一通道，而重组导航时
 * 「某个分支连同它的落点一起消失」是**静默**的：typecheck / lint / 既有用例一个都不会红，
 * 用户看到的只是「点了没反应」。
 *
 * **判定收在这里、而不是散在 `WorkspaceApp` 的那个内联处理器里**，是为了让"这条前缀
 * 落在哪一格"成为一个**可被直接断言的返回值**。此前守护只能去源码里搜字符串
 *（`expect(APP).toContain('go("knowledge", "themes_tags")')`）——那种断言证明的是"这一行
 * 还在"，证明不了"点下去真的到得了那一格"，而且它每次改动都得跟着改字面量，最终会被改
 * 成与实现同义反复。
 *
 * 落点一律经 [`locationFor`] 归一（唯一入口），故不存在"某条深链带着别的空间的分区"这种
 * 只在特定跳转顺序下才复现的状态。
 */

import { parseNoteRoute } from "../../lib/note-path";
import { locationFor, type WorkspaceLocation } from "./workspace-ia";

/** 项目工作区的页签（ADR-165 ⑥）。录制与分发随后续批次加入。 */
export type CreatorTab = "overview" | "research" | "script" | "recording" | "publish" | "distribute";

export const CREATOR_TABS: readonly CreatorTab[] = ["overview", "research", "script", "recording", "publish", "distribute"];

/** 创作空间里的一处（深链能到的那几处）。 */
export type CreatorTarget =
  | { view: "board" }
  | { view: "swipes" }
  | { view: "project"; home: string; tab: CreatorTab };

/** 创作深链的唯一写法（与 [`parseWorkspaceRoute`] 互逆）。 */
export function creatorRoute(target: CreatorTarget): string {
  switch (target.view) {
    case "board":
      return "creator://";
    case "swipes":
      return "creator://swipes";
    case "project":
      return `creator://project/${target.home}#${target.tab}`;
  }
}

/** `creator://project/<主页>[#<页签>]` → 项目与页签；页签缺省或不认识时落概览。 */
function creatorTargetOf(route: string): CreatorTarget {
  const rest = route.slice("creator://".length);
  if (rest === "swipes") return { view: "swipes" };
  if (rest.startsWith("project/")) {
    const body = rest.slice("project/".length);
    const hash = body.lastIndexOf("#");
    const home = hash < 0 ? body : body.slice(0, hash);
    const raw = hash < 0 ? "" : body.slice(hash + 1);
    const tab = (CREATOR_TABS as readonly string[]).includes(raw) ? (raw as CreatorTab) : "overview";
    if (home !== "") return { view: "project", home, tab };
  }
  return { view: "board" };
}

/** 一条深链要求应用做的事。`location` = 这条深链的落点（没有落点的用 `null`）。 */
export type WorkspaceRouteIntent =
  /** 设置深链：`settings://<大类|分组>[/<键|分节>]` 原样交给设置目的地的唯一解析器。 */
  | { kind: "settings"; route: string; location: null }
  /** 知识体检报告（常驻宿主的浮层，不切目的地）。 */
  | { kind: "checkup"; location: null }
  /** 顶栏处理队列面板（同上，不切目的地）。 */
  | { kind: "queue"; location: null }
  /** 纯目的地跳转。 */
  | { kind: "goto"; location: WorkspaceLocation }
  /** 标签：落「知识库 → 标签」，带标签时顺带选中它。 */
  | { kind: "tag"; tag: string | null; location: WorkspaceLocation }
  /** 图谱：落「知识库 → 知识网络」，带路径时进入 1 跳局部聚焦。 */
  | { kind: "graph"; path: string | null; location: WorkspaceLocation }
  /**
   * 录音 / 转写：各落自己的一级空间，**带路径时顺带选中那一条**（与 `tag` / `graph`
   * 同形：落点 + 可选载荷）。
   *
   * 载荷不得静默丢弃：只切格不选中，用户看到的是一张列表加一句「选择一个录音以回放」，
   * 而调用方以为自己送用户去看某一篇。带不带对象是两种意图，解析器得能表达。
   */
  | { kind: "recording"; path: string | null; location: WorkspaceLocation }
  | { kind: "transcript"; path: string | null; location: WorkspaceLocation }
  /** 速记：落「速记」，带路径时顺带选中它（语音速记 HUD 存完后的「打开速记」，PRD-02-020）。 */
  | { kind: "quick_note"; path: string | null; location: WorkspaceLocation }
  /** 对话：落主工作区「对话」，带路径时载入快捷面板移交的同一场会话。 */
  | { kind: "chat"; path: string | null; location: WorkspaceLocation }
  /** 创作（ADR-162 / ADR-165 ⑥）：看板、灵感库，或某个项目的某个页签。 */
  | { kind: "creator"; target: CreatorTarget; location: WorkspaceLocation }
  /** 提词器（ADR-176 ①）：落提词器首页。 */
  | { kind: "teleprompter"; location: WorkspaceLocation }
  /** 应用（ADR-165 ⑤）：不带 id 落列表，带 id 落那个应用的详情页。 */
  | { kind: "apps"; appId: string | null; location: WorkspaceLocation }
  /** 笔记：落「笔记」，`target` 为空表示这条 `notes://` 解析不出对象。 */
  | { kind: "note"; target: { path: string; line: number | null } | null; location: WorkspaceLocation }
  /**
   * 知识产物：落「知识库 → 主题页」，带路径时顺带选中那一页（ADR-096 ③——主题页/
   * 知识地图归知识库空间，合成完成通知与产物深链都走这条）。路径裸传，与 `notes://`
   * 同口径（Rust 发出的库相对路径，不 decode）。
   */
  | { kind: "topic"; path: string | null; location: WorkspaceLocation };

/**
 * 解析一条深链；`null` = **无人受理**（未知 scheme 一律如此）。
 *
 * **`activity://` 与其余退役 scheme 走的就是这条 `null`**，不做静默重定向（ADR-046
 * 无兼容层）：留一条"自动改送到用量页"的分支，等于让一个已经不存在的心智继续活着，
 * 而调用方永远不会发现自己用的是旧路由。Rust 侧的同一条裁决由白名单负责
 * （`workspace_route_allowed`，`ROUTE_REJECTED`）。
 *
 * **`workspace://` 在前端也没有分支**：它的全部语义就是"唤起主窗口"，而那件事在
 * Rust 侧已经做完了；事件到达前端时无事可做。
 */
/**
 * 「当前那一个」哨兵——`queue://current` / `workspace://current` 里它一直是这个意思，
 * 那两条也从不把它当路径读。
 */
const CURRENT_SENTINEL = "current";

/**
 * 取一条对象深链里的**具体对象路径**；没有具体对象时给 `null`（= 落到那个空间本身）。
 *
 * **`current` 不是路径**。`recording://current` 是二次转写完成通知的目标（PRD-15-050
 * 禁止在通知里带文件名，故它只能说"刚才那一个"）；而这里此前原样把 `current` 当成库内
 * 相对路径去打开，主区当场报「找不到这篇笔记，它可能已被移动或删除：current」——一条
 * **永远走不通**的「前往」（真机 2026-09-09）。何况一条历史通知里的"当前"本就没有确定
 * 指向：那次录音在几周前，"当前"早已是别的东西了。
 *
 * 落到空间本身是这里唯一诚实的答案：把人送到录音清单，让他自己挑，而不是送进一个错误页。
 */
function objectPathOf(route: string, scheme: string): string | null {
  const path = route.slice(scheme.length);
  return path === "" || path === CURRENT_SENTINEL ? null : path;
}

export function parseWorkspaceRoute(route: string): WorkspaceRouteIntent | null {
  if (route.startsWith("settings://")) {
    return { kind: "settings", route, location: null };
  }
  if (route.startsWith("checkup://")) {
    return { kind: "checkup", location: null };
  }
  if (route.startsWith("queue://")) {
    return { kind: "queue", location: null };
  }
  // **笔记路径一律裸传，不 `decodeURIComponent`**——与 `notes://` 同口径（两者都是
  // Rust 发出的笔记相对路径）。`graph://` / `tags://` 那两条是编码的，但它们传的是标签名
  // 与聚焦目标，不是这条通道。混用的代价很具体：文件名里一个字面 `%` 就会让解码当场抛错。
  if (route.startsWith("recording://")) {
    return {
      kind: "recording",
      path: objectPathOf(route, "recording://"),
      location: locationFor("recordings"),
    };
  }
  if (route.startsWith("transcribe://")) {
    return {
      kind: "transcript",
      path: objectPathOf(route, "transcribe://"),
      location: locationFor("transcripts"),
    };
  }
  if (route.startsWith("quick-notes://")) {
    return {
      kind: "quick_note",
      path: objectPathOf(route, "quick-notes://"),
      location: locationFor("quick_notes"),
    };
  }
  if (route.startsWith("chats://")) {
    return {
      kind: "chat",
      path: objectPathOf(route, "chats://"),
      location: locationFor("chat"),
    };
  }
  if (route.startsWith("creator://")) {
    return { kind: "creator", target: creatorTargetOf(route), location: locationFor("creator") };
  }
  if (route.startsWith("teleprompter://")) {
    return { kind: "teleprompter", location: locationFor("teleprompter") };
  }
  if (route.startsWith("apps://")) {
    return {
      kind: "apps",
      appId: objectPathOf(route, "apps://"),
      location: locationFor("apps"),
    };
  }
  if (route.startsWith("topics://")) {
    // 主题页分区（ADR-088 的一格 / ADR-096 ③）：不带路径 = 落目录本身；带路径 =
    // 顺带选中那一页产物（合成完成通知、产物深链的形态）。
    const path = route.slice("topics://".length);
    return { kind: "topic", path: path || null, location: locationFor("knowledge", "topics") };
  }
  if (route.startsWith("tags://")) {
    // 语义**逐字不变**：拆分前它落 `knowledge/tags`，拆分后仍落 `knowledge/tags`。
    // 变的只是那一格从合并分区的一半变成一个独立分区，故编辑器内联标签点击与图谱标签
    // 节点跳转都不必改写目标。
    const tag = decodeURIComponent(route.slice("tags://".length));
    return { kind: "tag", tag: tag || null, location: locationFor("knowledge", "tags") };
  }
  if (route.startsWith("graph://")) {
    const path = decodeURIComponent(route.slice("graph://".length));
    return { kind: "graph", path: path || null, location: locationFor("knowledge", "network") };
  }
  if (route.startsWith("notes://")) {
    // 行锚（`#L12`）由问答出处跳转带来（PRD-16-012）：解析不出对象时仍然落到编辑面
    // ——「点了没反应」比「落到了笔记但没打开那一篇」更难归因。
    return {
      kind: "note",
      target: parseNoteRoute(route),
      location: locationFor("notes"),
    };
  }
  return null;
}
