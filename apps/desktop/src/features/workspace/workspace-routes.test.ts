/**
 * 深链 → 落点（ADR-087 ② / ADR-088）。
 *
 * **这一族此前是 22 条源码字符串扫描**（`expect(APP).toContain('go("knowledge",
 * "themes_tags")')` 这类）。那种断言的问题不是"不严"，而是**它证明的不是那件事**：
 * 它只说明源码里还有那一行，说明不了点下去到得了那一格；而每次改动都得跟着改字面量，
 * 于是它最终只会与实现同义反复。本批把判定抽成纯函数 [`parseWorkspaceRoute`]，落点因此
 * 成为一个可直接断言的**返回值**——下面每一条都在问"这条深链把用户送到哪儿"。
 *
 * 落点一律与 `WORKSPACE_IA` 派生的 `locationFor` 比对，不硬写 `{space, section}` 字面量：
 * 硬写的话，哪天分区改名而这里没改，用例会红在一个与用户无关的地方。
 */

import { describe, expect, it } from "vitest";
import { locationFor } from "./workspace-ia";
import { creatorRoute, parseWorkspaceRoute } from "./workspace-routes";

describe("深链落点", () => {
  it("录音与转写各是一级空间、各有自己的前缀；不带对象时就是纯落点", () => {
    expect(parseWorkspaceRoute("recording://")).toEqual({
      kind: "recording",
      path: null,
      location: locationFor("recordings"),
    });
    expect(parseWorkspaceRoute("transcribe://")).toEqual({
      kind: "transcript",
      path: null,
      location: locationFor("transcripts"),
    });
  });

  it("录音 / 转写深链带路径时**把对象一起交出去**，不静默丢弃", () => {
    // 只切格不选中，调用方以为自己送用户去看某一篇，用户看到的却是一张列表加一句
    // 「选择一个录音以回放」。
    expect(parseWorkspaceRoute("recording://Recordings/会议.md")).toEqual({
      kind: "recording",
      path: "Recordings/会议.md",
      location: locationFor("recordings"),
    });
    expect(parseWorkspaceRoute("transcribe://Transcripts/访谈.md")).toMatchObject({
      kind: "transcript",
      path: "Transcripts/访谈.md",
      location: locationFor("transcripts"),
    });
  });

  it("`current` 是哨兵不是路径——落空间本身，绝不去开一篇叫 current 的笔记", () => {
    // 二次转写完成通知的目标是 `recording://current`（PRD-15-050 禁止在通知里带文件名，
    // 它只能说"刚才那一个"）。此前这里原样当路径读，主区当场报「找不到这篇笔记：
    // current」——一条**永远走不通**的「前往」（真机 2026-09-09）。何况历史通知里的
    // "当前"本就没有确定指向：那次录音在几周前，"当前"早已是别的东西。
    expect(parseWorkspaceRoute("recording://current")).toEqual({
      kind: "recording",
      path: null,
      location: locationFor("recordings"),
    });
    expect(parseWorkspaceRoute("transcribe://current")).toMatchObject({ path: null });
    expect(parseWorkspaceRoute("quick-notes://current")).toMatchObject({ path: null });
  });

  it("名字里含 current 的真实笔记不受哨兵影响", () => {
    // 判据是**整段相等**，不是包含——否则 `Recordings/current.md` 这种再普通不过的
    // 文件名会被当成哨兵，用户点「前往」永远到不了自己那篇。
    expect(parseWorkspaceRoute("recording://Recordings/current.md")).toMatchObject({
      path: "Recordings/current.md",
    });
    expect(parseWorkspaceRoute("recording://current/一.md")).toMatchObject({
      path: "current/一.md",
    });
  });

  it("路径裸传，不做 URI 解码——与 `notes://` 同口径", () => {
    // 文件名里一个字面 `%` 就会让 `decodeURIComponent` 当场抛错；而这条通道传的是
    // Rust 发来的笔记相对路径，本来就没有编码过。
    expect(parseWorkspaceRoute("recording://Recordings/100%25 完成.md")).toMatchObject({
      path: "Recordings/100%25 完成.md",
    });
  });

  it("`topics://` 落「知识库 → 主题页」；带路径时顺带选中那一页产物（ADR-096 ③）", () => {
    expect(parseWorkspaceRoute("topics://")).toEqual({
      kind: "topic",
      path: null,
      location: locationFor("knowledge", "topics"),
    });
    // 路径裸传（与 `notes://` 同口径，Rust 发出的库相对路径不 decode）。
    expect(parseWorkspaceRoute("topics://Notes/项目 X.md")).toEqual({
      kind: "topic",
      path: "Notes/项目 X.md",
      location: locationFor("knowledge", "topics"),
    });
  });

  it("`tags://` 的语义逐字不变：拆分前后都落「知识库 → 标签」", () => {
    // 分区拆分之后那一格从合并分区的一半变成一个独立分区，但**目标没变**——编辑器内联
    // 标签点击与图谱标签节点跳转因此都不必改写。
    expect(parseWorkspaceRoute("tags://%E9%A1%B9%E7%9B%AE")).toEqual({
      kind: "tag",
      tag: "项目",
      location: locationFor("knowledge", "tags"),
    });
    // 空串 = 只去那一格，不选中任何标签。
    expect(parseWorkspaceRoute("tags://")).toEqual({
      kind: "tag",
      tag: null,
      location: locationFor("knowledge", "tags"),
    });
  });

  it("拆分前那个合并分区不是合法 section id——由类型层保证，不需要运行时断言", () => {
    // 深链只有 `tags://`，合并分区的 id 一次都没有进过路由白名单，故**没有「旧深链
    // 退役」这件事**：这里不写它的负向用例，也不加任何拒绝分支——为一个从未存在的
    // 字符串写一条"必须被拒绝"，只会让读到的人以为它曾经可用过（TDD-07 §4）。
    // 它不是合法 section id 这件事由 `WorkspaceSection` 这个封闭联合给：
    // `locationFor("knowledge", "themes_tags")` 当场编译不过。
    expect(locationFor("knowledge").section).toBe("topics");
  });

  it("`graph://` 解码路径后进入局部聚焦，并落「知识库 → 知识网络」", () => {
    expect(parseWorkspaceRoute("graph://Notes/%E7%A4%BA%E4%BE%8B.md")).toEqual({
      kind: "graph",
      path: "Notes/示例.md",
      location: locationFor("knowledge", "network"),
    });
    expect(parseWorkspaceRoute("graph://")).toMatchObject({ kind: "graph", path: null });
  });

  it("`notes://` 经唯一解析器取对象，并落「笔记 → 全部笔记」", () => {
    expect(parseWorkspaceRoute("notes://Notes/示例.md#L12")).toEqual({
      kind: "note",
      target: { path: "Notes/示例.md", line: 12 },
      location: locationFor("notes"),
    });
    // 解析不出对象时**仍然**落到编辑面：「点了没反应」比「到了笔记但没打开那一篇」
    // 更难归因。
    expect(parseWorkspaceRoute("notes://")).toEqual({
      kind: "note",
      target: null,
      location: locationFor("notes"),
    });
  });

  it("浮层类深链不切目的地（体检报告 / 处理队列 / 设置）", () => {
    expect(parseWorkspaceRoute("checkup://")).toEqual({ kind: "checkup", location: null });
    expect(parseWorkspaceRoute("queue://current")).toEqual({ kind: "queue", location: null });
    expect(parseWorkspaceRoute("settings://ai_services/usage")).toEqual({
      kind: "settings",
      route: "settings://ai_services/usage",
      location: null,
    });
  });

  it("退役与无落点的 scheme 一律无人受理，绝不静默重定向", () => {
    // 「动态」聚合页退役后，通知与用量各自回到了自己的宿主。留一条"自动改送到用量页"
    // 的兼容分支会让一个已经不存在的心智继续活着（ADR-046 无兼容层）。
    for (const route of ["activity://", "activity://usage", "activity://notifications"]) {
      expect(parseWorkspaceRoute(route), `这条路由必须无人受理：${route}`).toBeNull();
    }
    // `workspace://` 的全部语义就是"唤起主窗口"，那件事 Rust 侧已经做完了。
    expect(parseWorkspaceRoute("workspace://current")).toBeNull();
    expect(parseWorkspaceRoute("https://example.com")).toBeNull();
    expect(parseWorkspaceRoute("")).toBeNull();
  });
});

describe("速记深链（PRD-02-020 存完后的「打开速记」）", () => {
  it("quick-notes://<path> 落「速记」并选中那一条；空路径只切格", () => {
    expect(parseWorkspaceRoute("quick-notes://QuickNotes/语音速记 2026-09-03 1452.md")).toEqual({
      kind: "quick_note",
      path: "QuickNotes/语音速记 2026-09-03 1452.md",
      location: locationFor("quick_notes"),
    });
    expect(parseWorkspaceRoute("quick-notes://")).toEqual({
      kind: "quick_note",
      path: null,
      location: locationFor("quick_notes"),
    });
  });
});

describe("创作深链（ADR-162）", () => {
  it("apps:// 落「应用」列表；带 id 时落那个应用的详情（ADR-165 ⑤）", () => {
    expect(parseWorkspaceRoute("apps://")).toEqual({
      kind: "apps",
      appId: null,
      location: locationFor("apps"),
    });
    expect(parseWorkspaceRoute("apps://creator")).toEqual({
      kind: "apps",
      appId: "creator",
      location: locationFor("apps"),
    });
  });

  it("creator:// 落看板；swipes 落灵感库；不认识的写法同样落看板", () => {
    for (const route of ["creator://", "creator://current", "creator://Creator/第一期/第一期.md"]) {
      expect(parseWorkspaceRoute(route)).toEqual({
        kind: "creator",
        target: { view: "board" },
        location: locationFor("creator"),
      });
    }
    expect(parseWorkspaceRoute("creator://swipes")).toMatchObject({ target: { view: "swipes" } });
  });

  it("creator://project/<主页>#<页签> 落项目的那个页签；页签缺省或不认识落概览（ADR-165 ⑥）", () => {
    const home = "Creator/第一期/第一期.md";
    expect(parseWorkspaceRoute(`creator://project/${home}#script`)).toMatchObject({
      target: { view: "project", home, tab: "script" },
    });
    expect(parseWorkspaceRoute(`creator://project/${home}`)).toMatchObject({
      target: { view: "project", home, tab: "overview" },
    });
    expect(parseWorkspaceRoute(`creator://project/${home}#bogus`)).toMatchObject({
      target: { view: "project", home, tab: "overview" },
    });
  });

  it("创作深链的写法与解析互逆（路径里带 # 也不丢）", () => {
    for (const target of [
      { view: "board" as const },
      { view: "swipes" as const },
      { view: "project" as const, home: "Creator/P #1/P #1.md", tab: "publish" as const },
    ]) {
      expect(parseWorkspaceRoute(creatorRoute(target))).toMatchObject({ target });
    }
  });
});

describe("对话深链（快捷面板转到主工作区继续）", () => {
  it("chats://<path> 落「对话」并携带同一场归档；空路径打开新对话页", () => {
    expect(parseWorkspaceRoute("chats://Chats/对话 2026-09-21 1030.md")).toEqual({
      kind: "chat",
      path: "Chats/对话 2026-09-21 1030.md",
      location: locationFor("chat"),
    });
    expect(parseWorkspaceRoute("chats://")).toEqual({
      kind: "chat",
      path: null,
      location: locationFor("chat"),
    });
  });
});
