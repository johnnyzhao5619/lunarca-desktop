/// <reference types="vite/client" />
/**
 * 空间表面与三个共用外壳的守护（PRD-07-071 / TDD-15 §3.3a）。
 *
 * 用户看到的"每个页风格不太一样"，在代码里从来不是一处大错，而是三处小分叉：
 * 纸张各画各的、页头各写各的、侧栏主按钮各有各的形态。三者都只有**结构性守护**拦得住
 * ——它们各自合法、各自只有一个定义点，靠 `css-selector-guard` 那一层永远发现不了。
 *
 * ① 每个空间都**声明**自己是纸还是画布。判据在注册表，不在样式里，也不看"它长得像什么"。
 * ② 文档面的主区根**挂 `.luna-paper` 原语**，不各写一份描边圆角底色。
 * ③ 主区页头**挂 `.luna-main-header` 原语**。
 * ④ 四条采集侧栏的主按钮**同一种形态**（`.luna-capture-start`）。
 */

import { describe, expect, it } from "vitest";
import { WORKSPACE_IA, surfaceOf, type WorkspaceSpace } from "./workspace-ia";

/**
 * 判据只看 `className="…"` 里出现的类，**不看整份源码文本**。
 *
 * 反向夹具当场抓到过这件事：本文件的注释里就写着这些类名，`toContain(源码, 类名)`
 * 于是被自己的注释喂饱，把类从 `className` 里删掉照样全绿。守护读错了地方，比没有守护
 * 更糟——它给的是一个假的"已经守住了"。
 */
function usesClass(source: string, className: string): boolean {
  return usesClassesTogether(source, [className]);
}

/**
 * **同一个** `className` 属性里同时挂着这几个类。
 *
 * 第二次反向夹具（2026-09-08 评审）：只查"文件里某处有 `luna-paper`"，把纸从根元素
 * 挪到任意子元素照样全绿。纸必须挂在**根**上，判据就得把根类与纸类绑在同一个属性里查。
 */
function usesClassesTogether(source: string, classNames: readonly string[]): boolean {
  return [...source.matchAll(/className="([^"{}]+)"/g)].some(([, value]) => {
    const list = value.split(/\s+/);
    return classNames.every((name) => list.includes(name));
  });
}

const TSX = import.meta.glob("../../{app,features,components}/**/*.tsx", {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

/**
 * 文档面空间的主区根组件：**注册表里每一个 `document` 空间** → 承载它的文件与根类。
 *
 * 这张表与注册表**互相锁死**（下方用例）：注册表多声明一个 `document` 空间而这里没有
 * 它的根，或这里列着一个注册表已改成 `canvas` 的空间，都判红。这就是"声明在注册表"与
 * "纸挂在组件根上"之间的机器关系——没有这条，`SpaceSpec.surface` 只是一句没人核对的话
 * （2026-09-08 评审指出 `data-surface` 无任何样式消费者，判据不自洽）。
 */
const DOCUMENT_ROOTS: Readonly<Partial<Record<WorkspaceSpace, readonly [string, string]>>> = {
  search: ["../editor/VaultEditor.tsx", "luna-editor"],
  notes: ["../editor/VaultEditor.tsx", "luna-editor"],
  quick_notes: ["../editor/VaultEditor.tsx", "luna-editor"],
  recordings: ["../playback/SyncPlaybackView.tsx", "luna-playback"],
  transcripts: ["../playback/SyncPlaybackView.tsx", "luna-playback"],
};

/** 挂了主区页头原语的表面。收编一处就加一行——它是清单，不是抽样。 */
const MAIN_HEADER_HOSTS = [
  "../chat/ChatView.tsx",
  "../playback/SyncPlaybackView.tsx",
  "../editor/VaultEditor.tsx",
] as const;

/** 四条采集侧栏的主按钮所在文件。 */
const CAPTURE_PRIMARY_BUTTONS = [
  "../../app/WorkspaceApp.tsx",
  "../../app/TranscribeNoteList.tsx",
  "../chat/ChatHistoryList.tsx",
  "../capture/CaptureBox.tsx",
] as const;

describe("空间表面 · 判据在注册表里", () => {
  it("守护本身确实看得到源码（防空集永远通过）", () => {
    expect(Object.keys(TSX).length).toBeGreaterThan(20);
    expect(WORKSPACE_IA.length).toBeGreaterThan(5);
  });

  it("每个空间都声明了表面类型（加一个空间忘不掉）", () => {
    for (const spec of WORKSPACE_IA) {
      expect(["document", "canvas"], `${spec.space} 的表面类型非法`).toContain(
        surfaceOf(spec.space),
      );
    }
  });

  it("主区把声明下发到 DOM（样式据此收敛，不靠各页自觉）", () => {
    const shell = TSX["../../app/WorkspaceApp.tsx"];
    expect(shell).toContain("data-surface={surfaceOf(space)}");
  });

  it("注册表里的 document 空间 = 有根组件挂纸的空间，一个不多一个不少", () => {
    const declared = WORKSPACE_IA.filter((spec) => surfaceOf(spec.space) === "document")
      .map((spec) => spec.space)
      .sort();
    const listed = Object.keys(DOCUMENT_ROOTS).sort();
    expect(listed, "注册表声明了 document 却没有登记它的主区根（或反之）").toEqual(declared);
  });
});

describe("三个共用外壳 · 不得再各写一份", () => {
  it.each(Object.entries(DOCUMENT_ROOTS))("%s 空间的主区根挂 .luna-paper", (space, root) => {
    const [path, rootClass] = root;
    const source = TSX[path];
    expect(source, `${space}：找不到 ${path}`).toBeDefined();
    expect(
      usesClassesTogether(source, [rootClass, "luna-paper"]),
      `${rootClass} 应与 .luna-paper 挂在同一个根上——纸张只有一个定义点，也只能在根上`,
    ).toBe(true);
  });

  it.each(MAIN_HEADER_HOSTS)("%s 的页头挂 .luna-main-header", (path) => {
    const source = TSX[path];
    expect(source, `找不到 ${path}`).toBeDefined();
    expect(usesClass(source, "luna-main-header"), "主区页头只有一个定义点").toBe(true);
  });

  it.each(CAPTURE_PRIMARY_BUTTONS)("%s 的采集主按钮走同一枚组件", (path) => {
    const source = TSX[path];
    expect(source, `找不到 ${path}`).toBeDefined();
    // 2026-09-09：判据从"挂了同一个类名"收紧成"用了同一枚组件"。类名相同只保证长得像，
    // 而那四处此前各写一遍同样的 button，**快捷键提示只有速记那一颗有**——用户在录音与
    // 对话那两格里根本看不出这两件事也能在别的应用里按。
    expect(
      source.includes("<CaptureStartButton"),
      "四条采集侧栏的主按钮是同一种东西，不另写同义实现",
    ).toBe(true);
    expect(
      usesClass(source, "luna-capture-start"),
      "类名归组件，调用点不再自己写——写了就是又一份同义实现",
    ).toBe(false);
  });

  it("那枚组件本身挂着共用形态，且快捷键提示是它的一部分", () => {
    const button = TSX["../../components/ui/CaptureStartButton.tsx"];
    expect(button, "找不到 CaptureStartButton").toBeDefined();
    expect(button).toContain('"luna-capture-start"');
    expect(button).toContain("luna-capture-start__kbd");
    // 读屏名要带上快捷键：`kbd` 是 `aria-hidden` 的装饰，名字里不提就等于没告诉读屏用户。
    expect(button).toContain('t("action.withShortcut"');
  });
});
