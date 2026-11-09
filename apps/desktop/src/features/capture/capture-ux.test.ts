// @vitest-environment happy-dom
/// <reference types="vite/client" />
/**
 * 记录界面的文案与命令收敛守护（PRD-06-058 P0-1 / P0-2 / P1-1，2026-08-24 真机验收）。
 *
 * 三件事在单元测试里各自都"看不见"，只有真机第一屏才暴露，故都在这里变成机器事实：
 *
 * - ① 落盘只有一条命令：前端一旦按目的地在两条命令之间 if/else，业务判定就搬进了
 *   界面层（ADR-003），而两个入口从此可以各自漂——真机上的表现正是"同屏两个输入口、
 *   落盘语义相反"。
 * - ② 捕获框占位只有一句话：占位此前塞进了快捷键教学，在侧栏宽度下折行并被裁掉半行。
 *   反向条款同样重要——教学不是被删了，是搬到了脚注键上，那一条必须仍然讲得全。
 * - ③ 画布三处文案按目的地分化：它们走模板串取键（`canvas.${destination}.…`），
 *   静态 key 守护（`i18n/static-keys.test.ts`）看不到也不该假装看得到，故这里补上
 *   它明确让位的那份"专属守护"。
 *
 * 批次 E 把 ① ② 从源码字符串断言换成真实事实**：
 * ① 此前比对的是 `bindings.ts` 的原始文本（"文件里还有没有 `captureQuickNote` 这个
 *    词"）——那既拦不住"命令还在、只是没人调"，也会被一次格式化误红。现在直接问那个
 *    真的被导入的 `commands` 对象：退役的命令不是键，收敛后的那条是可调用函数且收两个参数。
 * ② 此前比对的是语言包里的字符串。现在把 `CaptureBox` 真的渲染出来，读它 DOM 上的
 *    `placeholder` 属性——中间任何一环（忘了传 `placeholder`、传错 key）断掉都会红，
 *    而语言包断言对这些一无所知。顺带把这个控件的键盘契约（回车落袋 / ⌘回车展开 /
 *    组字期间不提交 / 存不下不清空）一并变成可观察行为：它是这个控件的全部产品面，
 *    此前一条用例都没有。
 */

import { createElement } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, waitFor } from "@testing-library/react";

import { commands } from "../../ipc/bindings";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { CANVAS_DESTINATIONS } from "../workspace/workspace-ia";
import { COMPOSITION_COMMIT_WINDOW_MS } from "@lunarca/editor-core/ime-composition";
import { CaptureBox } from "./CaptureBox";

const LOCALES = import.meta.glob("../../i18n/locales/*/*.json", {
  eager: true,
}) as Record<string, { default: Record<string, unknown> }>;

/** `locale → namespace → 解析后的对象`。 */
function bundle(locale: string, ns: string): Record<string, unknown> {
  const hit = Object.entries(LOCALES).find(([path]) =>
    path.endsWith(`/locales/${locale}/${ns}.json`),
  );
  if (!hit) {
    throw new Error(`找不到语言包 ${locale}/${ns}.json`);
  }
  return hit[1].default;
}

/** 按点号路径取值；取不到返回 `undefined`（守护据此判红）。 */
function lookup(root: Record<string, unknown>, path: string): unknown {
  return path
    .split(".")
    .reduce<unknown>(
      (node, key) =>
        typeof node === "object" && node !== null
          ? (node as Record<string, unknown>)[key]
          : undefined,
      root,
    );
}

const LOCALE_CODES = ["zh-CN", "en-US"] as const;

/** 快捷键教学的形状（任一语言里都算数）。 */
const SHORTCUT_TOKENS = ["⌘", "Ctrl", "Shift", "Enter", "回车"];

/**
 * 近似渲染宽度（CJK 记 2 格，其余记 1 格）——"会不会在侧栏里折行"看的是宽度，不是字数。
 * 拿字数当判据会让中英两套文案适用两个尺子：`随手记点什么…` 才 7 个字，却比 19 个字符的
 * `Jot something down…` 还宽。
 */
function displayWidth(text: string): number {
  return [...text].reduce((sum, ch) => sum + (ch.codePointAt(0)! > 0x2e80 ? 2 : 1), 0);
}

/** 侧栏最窄档（180px）下占位一行放得下的宽度上限。修复前那句约 40 格，被裁掉半行。 */
const PLACEHOLDER_MAX_WIDTH = 34;

afterEach(cleanup);

describe("落盘命令收敛（PRD-06-058 P0-1）", () => {
  it("退役的两条命令不得复活——文本落盘只有 `createNoteFromText` 一条", () => {
    // 它俩此前一条落记录、一条落知识笔记，而调用点各自决定叫哪条：那就是"前端在挑
    // 笔记类型"。收成一条之后，前端交出去的只有正文与目的地这一事实。
    expect(commands).not.toHaveProperty("captureQuickNote");
    expect(commands).not.toHaveProperty("saveEditorDraftToNote");
    expect(typeof commands.createNoteFromText).toBe("function");
  });

  it("该命令必须收下目的地参数——没有它就退回成「前端替 Rust 决定形态」", () => {
    // 形参个数是运行时可观察的：正文一个、目的地一个，少一个就说明目的地又被前端吞了。
    expect(commands.createNoteFromText).toHaveLength(2);
  });
});

describe("快速捕获按需展开（2026-10-08 R1）", () => {
  function mountCollapsed() {
    const onCapture = vi.fn(async () => true);
    const view = render(
      withI18n(createElement(CaptureBox, { onCapture, onVoiceCapture: vi.fn(), voiceShortcut: "" })),
    );
    return { view, onCapture };
  }

  it("默认不常驻输入框：只有语音主按钮与「快速捕获」次动作", () => {
    const { view } = mountCollapsed();
    expect(view.container.querySelector("textarea")).toBeNull();
    expect(view.getByRole("button", { name: t("workspace:draft.captureOpen") })).toBeTruthy();
  });

  it("点开即聚焦；空框按 Esc 收起，焦点回到「快速捕获」", async () => {
    const { view } = mountCollapsed();
    fireEvent.click(view.getByRole("button", { name: t("workspace:draft.captureOpen") }));
    const input = view.container.querySelector("textarea")!;
    await waitFor(() => expect(document.activeElement).toBe(input));
    fireEvent.keyDown(input, { key: "Escape" });
    expect(view.container.querySelector("textarea")).toBeNull();
    await waitFor(() =>
      expect(document.activeElement?.textContent).toContain(t("workspace:draft.captureOpen")),
    );
  });

  it("框里有字时不收起：Esc 无效、收起键禁用——不静默丢掉仅有的那一份", () => {
    const { view } = mountCollapsed();
    fireEvent.click(view.getByRole("button", { name: t("workspace:draft.captureOpen") }));
    const input = view.container.querySelector("textarea")!;
    fireEvent.change(input, { target: { value: "还没存的一句" } });
    fireEvent.keyDown(input, { key: "Escape" });
    expect(view.container.querySelector("textarea")?.value).toBe("还没存的一句");
    const close = view.getByRole("button", { name: t("workspace:draft.captureCollapse") }) as HTMLButtonElement;
    expect(close.disabled).toBe(true);
  });
});

describe("捕获框：真实渲染下的占位与键盘契约（PRD-06-058 P0-2）", () => {
  function mount(onCapture = vi.fn(async () => true), onVoiceCapture = vi.fn()) {
    const view = render(
      withI18n(
        createElement(CaptureBox, { onCapture, onVoiceCapture, voiceShortcut: "⌥⌘V", openRequest: 1 }),
      ),
    );
    const input = view.container.querySelector("textarea")!;
    return { view, input, onCapture, onVoiceCapture };
  }

  it("占位就是语言包里那一句，且**真的挂在 DOM 上**", () => {
    const { input } = mount();
    expect(input.getAttribute("placeholder")).toBe(t("workspace:draft.capturePlaceholder"));
    expect(input.getAttribute("aria-label")).toBe(t("workspace:draft.captureLabel"));
  });

  it("教学不在占位里，而在 `aria-describedby` 指向的脚注里", () => {
    // 两条通道必须说同一句话：读屏用户聚焦时听到的，就是视觉上展开的那一行。接线断了
    // （id 改了、describedby 没跟上）在这里当场红。
    const { view, input } = mount();
    const hintId = input.getAttribute("aria-describedby");
    expect(hintId).toBeTruthy();
    const hint = view.container.querySelector(`#${hintId}`);
    // 完整三条必须恒在 DOM 里**：脚注视觉上是两段式（静息短句 / 聚焦展开），而那
    // 是纯 CSS 的折叠。折叠一旦被写成"静息时不渲染那个 span"，读屏用户在静息态听到的
    // 就只剩「回车即存」——视觉折叠悄悄变成了信息删减，而这正是这条用例守的东西。
    expect(hint?.textContent).toContain(t("workspace:draft.captureHint"));
    expect(hint?.textContent).toContain(t("workspace:draft.captureHintShort"));
    for (const token of SHORTCUT_TOKENS) {
      expect(input.getAttribute("placeholder")).not.toContain(token);
    }
  });

  it("静息态也看得见「这里能打字」——短句不藏在 `:focus-within` 后面", () => {
    // 2026-09-09 评审：整条脚注此前静息 `display: none`，于是"这里能打字"要先聚焦
    // 才看得到证据。短句是首屏最便宜的一次自我说明；它在不在 DOM 里是可断言的事实，
    // 而"静息时可见"是 CSS 的事（`.luna-capture-field__hint-short` 无隐藏规则）。
    const { view } = mount();
    const short = view.container.querySelector(".luna-capture-field__hint-short");
    expect(short?.textContent).toBe(t("workspace:draft.captureHintShort"));
  });

  it("提交按钮：鼠标用户看得见的那条「存下」路径", async () => {
    // 键盘契约再完备，鼠标用户也从来不知道要按回车——用户报的「没有文本速记的入口」
    // 有一半是这颗按钮不存在。
    const { view, input, onCapture } = mount();
    const submit = view.getByLabelText(t("workspace:draft.captureSubmit")) as HTMLButtonElement;
    expect(submit.disabled, "空内容时不给一颗按下去没反应的按钮").toBe(true);
    fireEvent.change(input, { target: { value: "点按钮存的这条" } });
    expect(submit.disabled).toBe(false);
    fireEvent.click(submit);
    // 与回车逐字等价：同一个 `open: false`、同样清空、同样把焦点留在框里。
    expect(onCapture).toHaveBeenCalledWith("点按钮存的这条", { open: false });
    await waitFor(() => expect(input.value).toBe(""));
    expect(document.activeElement, "焦点留在框里，下一条接着敲").toBe(input);
  });

  it("Enter = 存下并留在框里（连续捕获零决策）", async () => {
    const { input, onCapture } = mount();
    fireEvent.change(input, { target: { value: "一条随手记" } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onCapture).toHaveBeenCalledWith("一条随手记", { open: false });
    await waitFor(() => expect(input.value).toBe(""));
    expect(document.activeElement, "焦点留在框里，下一条接着敲").toBe(input);
  });

  it("⌘/Ctrl+Enter = 存下并打开编辑器", async () => {
    const { input, onCapture } = mount();
    fireEvent.change(input, { target: { value: "要展开的这条" } });
    fireEvent.keyDown(input, { key: "Enter", metaKey: true });
    await waitFor(() => expect(onCapture).toHaveBeenCalledWith("要展开的这条", { open: true }));
  });

  it("Shift+Enter 换行——不提交，交给 textarea 原生行为", () => {
    const { input, onCapture } = mount();
    fireEvent.change(input, { target: { value: "第一行" } });
    const prevented = !fireEvent.keyDown(input, { key: "Enter", shiftKey: true });
    expect(onCapture).not.toHaveBeenCalled();
    expect(prevented, "换行不能被拦下来").toBe(false);
  });

  it("输入法组字期间的 Enter 一律不算提交", () => {
    // 中文用户每选一次候选词就落一篇半截笔记——本控件最容易犯、也最难被英文测试发现的错。
    const { input, onCapture } = mount();
    fireEvent.change(input, { target: { value: "zhong" } });
    fireEvent.keyDown(input, { key: "Enter", isComposing: true });
    expect(onCapture).not.toHaveBeenCalled();
  });

  it("WKWebView 顺序：`compositionend` 先于 `keydown`，那一下仍然不算提交", () => {
    // 真机 2026-09-09 的原样复现（侧栏与全局面板同时中招）：合成在 keydown 之前结束，
    // 于是那一下身上 `isComposing` 是假、keyCode 是 13——单看事件它就是一次普通回车，
    // 一段还没打完的拼音被落成了笔记。判据改由组件记下的合成边界补齐（`@lunarca/editor-core/ime-composition`）。
    const { input, onCapture } = mount();
    fireEvent.change(input, { target: { value: "zhong" } });
    fireEvent.compositionEnd(input, { data: "zhong" });
    fireEvent.keyDown(input, { key: "Enter", isComposing: false, keyCode: 13 });
    expect(onCapture, "上屏那一下被当成了提交").not.toHaveBeenCalled();
  });

  it("上屏那一下必须原样放行——吃掉它，候选词就永远上不了屏", () => {
    // 这条与上一条是一件事的两面：不提交且不 `preventDefault`。此前的实现是先无条件
    // 拦下再决定要不要提交，于是被判为「让给输入法」的那一下也已经被吃掉了。
    const { input } = mount();
    fireEvent.compositionEnd(input, { data: "zhong" });
    const notPrevented = fireEvent.keyDown(input, { key: "Enter", isComposing: false });
    expect(notPrevented, "事件被 preventDefault 了，输入法拿不到这一下").toBe(true);
  });

  it("上屏之后再按一次回车 = 真的要存那串字", async () => {
    // 反向条款：合成边界只用来认「同一次按键」。用户看着框里的内容又按了一次回车，
    // 那就是他真的要存它——把这一下也吞掉，中文用户就再也存不下东西了。
    //
    // 这里必须真的等而不能用假时钟：判据读的是事件自己的 `timeStamp`（单调时钟），
    // `vi.advanceTimersByTime` 推不动它——假时钟下这条用例只会证明"时间没走时不提交"。
    const { input, onCapture } = mount();
    fireEvent.change(input, { target: { value: "中" } });
    fireEvent.compositionEnd(input, { data: "中" });
    await new Promise((resolve) => setTimeout(resolve, COMPOSITION_COMMIT_WINDOW_MS * 2));
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(onCapture).toHaveBeenCalledWith("中", { open: false }));
  });

  it("没存成就不清空——用户刚敲的那段字是他仅有的一份", async () => {
    const onCapture = vi.fn(async () => false);
    const { input } = mount(onCapture);
    fireEvent.change(input, { target: { value: "写了一半" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(onCapture).toHaveBeenCalled());
    expect(input.value).toBe("写了一半");
  });

  it("空白内容不落盘（回车不该产生一篇空笔记）", () => {
    const { input, onCapture } = mount();
    fireEvent.change(input, { target: { value: "   " } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onCapture).not.toHaveBeenCalled();
  });

  it("按钮唤起既有语音入口，并在读屏名里带上全局快捷键（PRD-02-020）", () => {
    // 名字里带快捷键的模式归 `common:action.withShortcut` 一处——四条采集侧栏的主按钮
    // 用的是同一枚组件，此前只有速记那颗有这个提示。
    const { view, onVoiceCapture } = mount();
    const mic = view.getByLabelText(
      t("common:action.withShortcut", {
        action: t("workspace:capture.startVoiceInput"),
        shortcut: "⌥⌘V",
      }),
    );
    fireEvent.click(mic);
    expect(onVoiceCapture).toHaveBeenCalledTimes(1);
  });
});

describe("占位与教学的双语对照（PRD-06-058 P0-2）", () => {
  // 上一组证明"渲染时真的用了这条 key"，这一组证明"两种语言都写得下、都讲得全"——
  // 前者换语言不会变红，后者才是 parity。
  for (const locale of LOCALE_CODES) {
    const workspace = bundle(locale, "workspace");

    it(`${locale}：占位只有一句话，不塞快捷键教学（会折行被裁掉半行）`, () => {
      const placeholder = lookup(workspace, "draft.capturePlaceholder");
      expect(typeof placeholder).toBe("string");
      const text = placeholder as string;
      for (const token of SHORTCUT_TOKENS) {
        expect(text).not.toContain(token);
      }
      // 括号里塞第二句是它变长的老路——一句话就是一句话。
      expect(text).not.toMatch(/[（(]/u);
      expect(displayWidth(text)).toBeLessThanOrEqual(PLACEHOLDER_MAX_WIDTH);
    });

    it(`${locale}：反向条款——教学没有被删掉，三条键盘契约在脚注里讲全`, () => {
      const hint = lookup(workspace, "draft.captureHint");
      expect(typeof hint).toBe("string");
      const text = hint as string;
      // 存下 / 存下并展开 / 换行——少讲一条，这个控件就有一半的行为无处可学。
      expect(text).toContain("Shift");
      expect(text).toContain("⌘");
      expect(text).toContain("Ctrl");
    });
  }
});

describe("空白画布三处文案按目的地分化（PRD-06-058 P0-1 / P1-1）", () => {
  it("目的地清单非空且由 Rust 的 DTO 穷尽", () => {
    expect(CANVAS_DESTINATIONS.length).toBeGreaterThanOrEqual(2);
    expect([...CANVAS_DESTINATIONS]).toContain("drafts");
    expect([...CANVAS_DESTINATIONS]).toContain("notes");
  });

  for (const locale of LOCALE_CODES) {
    it(`${locale}：每个目的地都有自己的占位 / 空态 / 面包屑，一个都不缺`, () => {
      const editor = bundle(locale, "editor");
      const workspace = bundle(locale, "workspace");
      for (const destination of CANVAS_DESTINATIONS) {
        for (const leaf of ["placeholder", "empty"]) {
          expect(
            lookup(editor, `canvas.${destination}.${leaf}`),
            `editor:canvas.${destination}.${leaf}`,
          ).toEqual(expect.any(String));
        }
        expect(
          lookup(workspace, `canvas.noSelection.${destination}`),
          `workspace:canvas.noSelection.${destination}`,
        ).toEqual(expect.any(String));
      }
    });

    it(`${locale}：反向条款——两个目的地不得共用同一句话`, () => {
      // 共用即等于没分化：那正是修复前的状态（面包屑「未选择笔记」+ 空态「直接输入创建
      // 新笔记」在记录语境下承诺了一个相反的落点）。
      const editor = bundle(locale, "editor");
      const workspace = bundle(locale, "workspace");
      const copies = (leafPath: (d: string) => string, root: Record<string, unknown>) =>
        new Set(CANVAS_DESTINATIONS.map((d) => lookup(root, leafPath(d)) as string));
      expect(copies((d) => `canvas.${d}.placeholder`, editor).size).toBe(
        CANVAS_DESTINATIONS.length,
      );
      expect(copies((d) => `canvas.${d}.empty`, editor).size).toBe(CANVAS_DESTINATIONS.length);
      expect(copies((d) => `canvas.noSelection.${d}`, workspace).size).toBe(
        CANVAS_DESTINATIONS.length,
      );
    });
  }
});
