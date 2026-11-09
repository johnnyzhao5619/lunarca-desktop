/// <reference types="vite/client" />
/**
 * 常驻播放宿主的结构守护（ADR-118 / PRD-04-064/065）。
 *
 * 这条契约靠"记得住"是守不住的：媒体元素与播放状态一旦被谁顺手搬回视图里，界面照常работает
 * 一切正常——只有在**切走空间**的那一刻才会露出来（声音断掉），而 headless 测试跑不到那一刻。
 * 所以把不变式钉在结构上：
 *
 * ① `<video>` 全应用**只有一处**，且在宿主里。搬回视图 = 空间切换时随子树卸载 = 播放中断。
 * ② `usePlayback(` 只有宿主调用。视图再调一次会得到第二套播放状态与第二个 cue 指针。
 * ③ 宿主在 `WorkspaceApp` 的**空间条件渲染之外**（包着 `.luna-workspace` 根）。
 * ④ 画面层隐藏时**不得 `display: none`**——那会让 WebKit 停掉解码，"切走仍在放"当场失效。
 * ⑤ 出声互斥订阅三个来源（录音 / 语音 HUD / 朗读），且判据是 Rust 事件而非界面按钮。
 */

import { describe, expect, it } from "vitest";

const TSX = import.meta.glob(["../../{app,features,components}/**/*.tsx", "../../../../../packages/*/src/**/*.tsx"], {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

const TS_SOURCES = import.meta.glob(["../../{app,features,components}/**/*.{ts,tsx}", "../../../../../packages/*/src/**/*.{ts,tsx}"], {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

const EDITOR = import.meta.glob("../editor/*.{ts,tsx}", {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

const CSS = import.meta.glob("./*.css", {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

/**
 * 去掉注释之后的源码。
 *
 * 判据只能落在**代码**上：注释里写着 `<video>` 与 `timecodeGutterExtension(` 的地方
 * 多得很，讲的正是这些契约本身；反过来，把 `<video` 写在一行中间（`<div><video …/>`）
 * 或先 `const g = timecodeGutterExtension(…)` 再塞进扩展数组，都能绕过"行首"这种文本形状
 * 的判据（2026-09-08 评审反向夹具抓到）。`://` 不算行注释（`recording://` 这类字面量）。
 */
function withoutComments(text: string): string {
  return text.replace(/\/\*[\s\S]*?\*\//g, "").replace(/(^|[^:])\/\/[^\n]*/g, "$1");
}

/** glob 的键相对**本文件**，故本 feature 内的文件形如 `./x.tsx`；只比对文件名更稳。 */
function filesContaining(sources: Record<string, string>, needle: string): string[] {
  return Object.entries(sources)
    .filter(([, text]) => text.includes(needle))
    .map(([path]) => path.split("/").pop() ?? path)
    .sort();
}

describe("常驻播放宿主 · 结构不变式", () => {
  it("守护本身确实看得到源码（防空集永远通过）", () => {
    expect(Object.keys(TSX).length).toBeGreaterThan(20);
    expect(Object.keys(CSS).length).toBeGreaterThan(0);
  });

  it("回放的 `<video>` 元素只在宿主里创建（搬回视图 = 切空间即中断播放）", () => {
    // 判据是**去掉注释后的代码里**有没有 `<video` 元素：注释里写着它的地方多得很。
    const creators = Object.entries(TSX)
      .filter(([, text]) => /<video\b/.test(withoutComments(text)))
      .map(([path]) => path.split("/").pop() ?? path)
      // 资产预览是另一个物种：它放的是库里任意一个媒体文件，与同步回放的时间轴无关，
      // 也不需要跨空间续放。
      .filter((name) => name !== "AssetView.tsx")
      .sort();
    expect(creators).toEqual(["playback-host.tsx"]);
  });

  it("画面槽位的登记带身份（裸 `null` 会让旧槽位抹掉活着的那个）", () => {
    // ref 回调的 `null` 不携带身份，而分屏 / 剧场之间切布局时 React 可以先挂新槽位、
    // 再卸旧槽位——照单全收那一次迟到的 `null`，宿主就以为自己不在回放视图：既不给
    // 后台悬浮小窗，也不给「正在播放」条，屏幕上看起来是"切走之后什么都没了"。
    // 判定归 `applyStageRegistration`，此处只钉住两件结构事实。
    const host = withoutComments(TSX["./playback-host.tsx"]);
    expect(host, "宿主必须经 applyStageRegistration 落状态，不能自己写一份归属规则").toMatch(
      /applyStageRegistration\(/,
    );
    expect(
      host,
      "registerStage 必须收 stageId：没有身份就分不清这次 null 是谁的",
    ).toMatch(/registerStage\s*=\s*useCallback\(\s*\(\s*stageId/);

    // 槽位那一侧不得把宿主的 registerStage 直接当 ref 用（那正是没有身份的写法）。
    const stage = withoutComments(TSX["./VideoStage.tsx"]);
    expect(stage, "ref 必须是绑好身份的回调，不能是裸 registerStage").not.toMatch(
      /ref=\{registerStage\}/,
    );
  });

  it("`usePlayback(` 只有宿主调用（视图再调一次 = 第二套播放状态）", () => {
    const callers = filesContaining(TS_SOURCES, "usePlayback(").filter(
      (name) => name !== "use-playback.ts" && !name.endsWith(".test.ts"),
    );
    expect(callers).toEqual(["playback-host.tsx"]);
  });

  it("宿主包在工作区根之外（空间条件渲染只卸投影，不卸媒体）", () => {
    const shell = TSX["../../app/WorkspaceApp.tsx"];
    expect(shell).toBeDefined();
    const provider = shell.indexOf("<PlaybackProvider>");
    const workspace = shell.indexOf('<div className="luna-workspace"');
    expect(provider).toBeGreaterThanOrEqual(0);
    expect(workspace).toBeGreaterThan(provider);
  });

  it("画面层隐藏时不用 display:none（那会停掉解码）", () => {
    const css = CSS["./playback.css"];
    const hidden = /\.luna-playback-stage-layer\[data-visible="false"\]\s*{([^}]*)}/.exec(css);
    expect(hidden, "找不到画面层的隐藏规则").not.toBeNull();
    expect(hidden![1]).not.toMatch(/display\s*:\s*none/);
    expect(hidden![1]).toMatch(/visibility\s*:\s*hidden/);
  });

  it("出声互斥订阅录音 / 语音 HUD / 朗读三个 Rust 事件（PRD-04-065）", () => {
    const host = TSX["./playback-host.tsx"];
    for (const source of [
      "events.recordingStateChanged.listen",
      "events.voiceInputHudOpened.listen",
      "events.ttsState.listen",
    ]) {
      expect(host, `互斥漏了一个出声来源：${source}`).toContain(source);
    }
  });
});

describe("就地听 · 装订线只在有时间戳的笔记上出现（PRD-04-066）", () => {
  const editor = () => EDITOR["../editor/VaultEditor.tsx"];

  it("守护本身确实看得到编辑器源码", () => {
    expect(editor()).toBeDefined();
  });

  it("装订线装在隔间里，行表常驻", () => {
    const source = editor();
    // CM6 只要装了 gutter 扩展就一定渲染那一列容器——无条件挂上的代价是**没有时间戳
    // 的笔记**（速记、普通笔记）左侧凭空多一条竖白边（真机 2026-09-08「预期外的白框」）。
    // 故装订线必须可摘：它只能经隔间进场。
    expect(source).toContain("timecodeCompartmentRef.current.of([])");
    expect(source).toContain("timecodeCompartmentRef.current.reconfigure(");
    // 行表则必须常驻：它与写它的 effect 若在同一次事务里才被创建，新字段拿到的是空表，
    // 那条 effect 就此丢掉，装订线永远装不上。
    expect(source).toContain("timecodeLinesExtension()");
  });

  it("装订线只在 reconfigure 的条件分支里构造一次（别名、无条件段落都是回退）", () => {
    const source = withoutComments(editor());
    // 只允许构造**一次**，且那一次必须是 `reconfigure(hasTimecodeLines ? timecodeGutterExtension(`：
    // 多构造一次（不管挂哪儿）就等于有一条无条件的路把它装进了编辑器。
    const calls = source.match(/timecodeGutterExtension\(/g) ?? [];
    expect(calls, "装订线只能构造一次").toHaveLength(1);
    expect(source).toMatch(/\.reconfigure\(\s*hasTimecodeLines\s*\?\s*timecodeGutterExtension\(/);
  });
});
