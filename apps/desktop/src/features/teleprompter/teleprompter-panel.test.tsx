// @vitest-environment happy-dom
/**
 * 提词器面板（ADR-162 ⑤ C4a，PRD-19-016）：内容与偏好都来自 Rust，面板只渲染、只发命令。
 */

import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));
vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({ onDragDropEvent: () => Promise.resolve(() => {}) }),
}));
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({ startResizeDragging: vi.fn() }),
}));
vi.mock("../../hooks/use-window-drag", () => ({ useWindowDrag: () => ({ onPointerDown: vi.fn() }) }));
/** 记下面板建的每一条 Channel，测试里冒充 Rust 往里送消息。 */
const channels: { onmessage: ((message: unknown) => void) | null }[] = [];
vi.mock("@tauri-apps/api/core", () => ({
  Channel: class {
    onmessage: ((message: unknown) => void) | null = null;
    constructor() {
      channels.push(this);
    }
  },
}));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import type { SettingSpecDto } from "../../ipc/bindings";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { TeleprompterPanel, decodeReadAt, lineAtReadingLine, neighborLine, remainingMillis } from "./TeleprompterPanel";

afterEach(cleanup);

function numberSpec(key: string, value: number, min: number, max: number, step: number): SettingSpecDto {
  return {
    key,
    current_value: { kind: "int", value },
    control: { kind: "number", min, max, step, unit: "none", display_divisor: 1 },
  } as unknown as SettingSpecDto;
}

beforeEach(() => {
  channels.length = 0;
  resetBindingsDouble();
  setCommandReply("listSettings", {
    status: "ok",
    data: [
      numberSpec("teleprompter.font_size", 118, 20, 120, 2),
      numberSpec("teleprompter.line_width", 70, 40, 100, 5),
      numberSpec("teleprompter.speed", 40, 5, 200, 5),
      { key: "teleprompter.mirror", current_value: { kind: "bool", value: false }, control: { kind: "toggle" } } as unknown as SettingSpecDto,
      // 跟读默认关（ADR-176 ④）；本文件的跟读用例都在开着的前提下测，关着的那条单独测。
      { key: "teleprompter.follow_enabled", current_value: { kind: "bool", value: true }, control: { kind: "toggle" } } as unknown as SettingSpecDto,
    ],
  });
  setCommandReply("setSetting", { status: "ok", data: null });
  setCommandReply("teleprompterHide", undefined);
  setCommandReply("teleprompterContent", {
    status: "ok",
    data: {
      path: "Creator/P/P-脚本.md",
      title: "P-脚本",
      extended: false,
      anchor: null,
      blocks: [
        { heading: "开头", lines: [{ text: "你好。", marker: "口播", spans: [{ kind: "text" as const, text: "你好。" }], cue: false }, { text: "今天聊三件事", marker: null, spans: [{ kind: "text" as const, text: "今天聊三件事" }], cue: false }] },
        { heading: "结尾", lines: [] },
      ],
    },
  });
});

describe("提词器面板", () => {
  it("按段显示要念的文字，形式标记作小标签；没有打开脚本时给出说明", async () => {
    render(withI18n(createElement(TeleprompterPanel)));
    expect(await screen.findByText("你好。")).toBeTruthy();
    expect(screen.getByText("口播")).toBeTruthy();
    expect(screen.getByText("结尾")).toBeTruthy();
    expect(screen.getByText("P-脚本")).toBeTruthy();
    cleanup();
    setCommandReply("teleprompterContent", { status: "ok", data: { path: null, title: "", blocks: [] } });
    render(withI18n(createElement(TeleprompterPanel)));
    expect(await screen.findByText(t("workspace:teleprompter.panel.empty"))).toBeTruthy();
  });

  it("步进按规格的步长写设置并夹在上下限内；到顶的那一侧不可点", async () => {
    render(withI18n(createElement(TeleprompterPanel)));
    await screen.findByText("你好。");
    const size = t("workspace:teleprompter.panel.fontSize");
    await waitFor(() => expect(screen.getByText(`${size} 118`)).toBeTruthy());
    fireEvent.click(screen.getByRole("button", { name: t("workspace:teleprompter.panel.more", { what: size }) }));
    expect(commandMock("setSetting")).toHaveBeenCalledWith("teleprompter.font_size", { kind: "int", value: 120 });
    expect(
      (screen.getByRole("button", { name: t("workspace:teleprompter.panel.more", { what: size }) }) as HTMLButtonElement)
        .disabled,
    ).toBe(true);
    const speed = t("workspace:teleprompter.panel.speed");
    fireEvent.click(screen.getByRole("button", { name: t("workspace:teleprompter.panel.less", { what: speed }) }));
    expect(commandMock("setSetting")).toHaveBeenCalledWith("teleprompter.speed", { kind: "int", value: 35 });
  });

  it("镜像只翻文字；收起走 Rust 的隐藏命令", async () => {
    const { container } = render(withI18n(createElement(TeleprompterPanel)));
    await screen.findByText("你好。");
    fireEvent.click(screen.getByRole("button", { name: t("workspace:teleprompter.panel.mirror") }));
    expect(container.querySelector(".luna-teleprompter")?.hasAttribute("data-mirror")).toBe(true);
    expect(commandMock("setSetting")).toHaveBeenCalledWith("teleprompter.mirror", { kind: "bool", value: true });
    fireEvent.click(screen.getByRole("button", { name: t("workspace:teleprompter.panel.close") }));
    await waitFor(() => expect(commandMock("teleprompterHide")).toHaveBeenCalledTimes(1));
    // 没在跟读：不去停一个不存在的会话。
    expect(commandMock("recordingStop")).not.toHaveBeenCalled();
  });
});

describe("跟读", () => {
  it("跟读开关关着（默认）：面板上没有跟读控件", async () => {
    setCommandReply("listSettings", {
      status: "ok",
      data: [
        { key: "teleprompter.follow_enabled", current_value: { kind: "bool", value: false }, control: { kind: "toggle" } } as unknown as SettingSpecDto,
      ],
    });
    render(withI18n(createElement(TeleprompterPanel)));
    await screen.findByText("你好。");
    expect(screen.queryByRole("button", { name: t("workspace:teleprompter.panel.follow") })).toBeNull();
  });

  it("位置帧是段、行、字三个 u32 LE", () => {
    const frame = new Uint8Array([1, 0, 0, 0, 2, 0, 0, 0, 44, 1, 0, 0]).buffer;
    expect(decodeReadAt(frame)).toEqual({ block: 1, line: 2 });
    expect(decodeReadAt([1, 0, 0])).toBeNull();
  });

  it("开启后把念到的那一行标出来；手动滚动不可用；收起前先停会话", async () => {
    setCommandReply("teleprompterFollowStart", { status: "ok", data: { status: "recording" } });
    setCommandReply("recordingStop", { status: "ok", data: { status: "idle" } });
    const { container } = render(withI18n(createElement(TeleprompterPanel)));
    await screen.findByText("你好。");
    fireEvent.click(screen.getByRole("button", { name: t("workspace:teleprompter.panel.follow") }));
    expect(await screen.findByRole("button", { name: t("workspace:teleprompter.panel.followStop") })).toBeTruthy();
    expect(
      (screen.getByRole("button", { name: t("workspace:teleprompter.panel.play") }) as HTMLButtonElement).disabled,
    ).toBe(true);
    // 跟读只用本机识别（ADR-176 ④）：进行时写明。
    expect(screen.getByText(t("workspace:teleprompter.panel.followLocal"))).toBeTruthy();
    // 第二条 Channel 是位置：Rust 说念到了第 0 段第 1 行。
    channels[1].onmessage?.(new Uint8Array([0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0]).buffer);
    await waitFor(() =>
      expect(container.querySelector('[data-at="0-1"]')?.hasAttribute("data-current")).toBe(true),
    );
    fireEvent.click(screen.getByRole("button", { name: t("workspace:teleprompter.panel.close") }));
    await waitFor(() => expect(commandMock("teleprompterHide")).toHaveBeenCalledTimes(1));
    expect(commandMock("recordingStop")).toHaveBeenCalledTimes(1);
  });

  it("开不起来（麦克风被占等）如实说明，开关复位", async () => {
    setCommandReply("teleprompterFollowStart", { status: "error", error: "MIC_BUSY: x" });
    render(withI18n(createElement(TeleprompterPanel)));
    await screen.findByText("你好。");
    fireEvent.click(screen.getByRole("button", { name: t("workspace:teleprompter.panel.follow") }));
    await waitFor(() => expect(screen.getByRole("button", { name: t("workspace:teleprompter.panel.follow") })).toBeTruthy());
    expect(screen.getAllByRole("status").length).toBeGreaterThan(0);
  });

  it("麦克风被占着：命令照常返回但状态是 mic_busy，开关不打开并说明是谁占着", async () => {
    setCommandReply("teleprompterFollowStart", {
      status: "ok",
      data: { status: "mic_busy", message_key: "workspace:teleprompter.panel.followFailed" },
    });
    render(withI18n(createElement(TeleprompterPanel)));
    await screen.findByText("你好。");
    fireEvent.click(screen.getByRole("button", { name: t("workspace:teleprompter.panel.follow") }));
    expect(await screen.findByText(t("workspace:teleprompter.panel.followFailed"))).toBeTruthy();
    expect(screen.getByRole("button", { name: t("workspace:teleprompter.panel.follow") })).toBeTruthy();
  });

  /**
   * 造一个滚动区：位置按屏幕坐标给（滚动区自己在屏幕上 80px 处——上面压着折成两行的控制条，
   * 真机 2026-11-04 就是这一截让带到阅读线的行偏上、换行换段原地不动），`top` 是行在内容里的位置。
   */
  function scrollBox(scrollTop: number, lines: readonly (readonly [string, number])[]) {
    const box = document.createElement("div");
    const boxScreenTop = 80;
    Object.defineProperty(box, "scrollTop", { value: scrollTop });
    Object.defineProperty(box, "clientHeight", { value: 300 });
    box.getBoundingClientRect = () => ({ top: boxScreenTop }) as DOMRect;
    for (const [at, top] of lines) {
      const line = document.createElement("p");
      line.dataset.at = at;
      // offsetTop 相对整个浮窗（多出控制条那 80px）：判定若误用它就会错一行。
      Object.defineProperty(line, "offsetTop", { value: top + boxScreenTop });
      line.getBoundingClientRect = () => ({ top: boxScreenTop + top - scrollTop }) as DOMRect;
      box.append(line);
    }
    return box;
  }

  it("阅读线上的那一行：第一条越过阅读线之前的最后一行，按滚动区自己的坐标算", () => {
    // 阅读线在 100 + 300 / 3 = 200：0-1 已过线，1-0 还没到。
    const box = scrollBox(100, [["0-0", 0], ["0-1", 150], ["1-0", 260]]);
    expect(lineAtReadingLine(box)).toEqual({ block: 0, line: 1 });
    expect(lineAtReadingLine(document.createElement("div"))).toBeNull();
  });

  it("刚被带到阅读线上的那一行，平滑滚动差一两个像素也仍算在线上", () => {
    // 想把 1-0（内容里 300）带到阅读线：scrollTop 应为 200，平滑滚动停在 199。
    const box = scrollBox(199, [["0-0", 0], ["0-1", 150], ["1-0", 300], ["1-1", 380]]);
    expect(lineAtReadingLine(box)).toEqual({ block: 1, line: 0 });
  });

  it("「标记重来」把此刻那一行报给 Rust：跟读时是高亮行；攒下的次数显示在按钮上", async () => {
    setCommandReply("teleprompterFollowStart", { status: "ok", data: { status: "recording" } });
    setCommandReply("teleprompterMark", { status: "ok", data: 2 });
    const { container } = render(withI18n(createElement(TeleprompterPanel)));
    await screen.findByText("你好。");
    fireEvent.click(screen.getByRole("button", { name: t("workspace:teleprompter.panel.follow") }));
    await screen.findByRole("button", { name: t("workspace:teleprompter.panel.followStop") });
    channels[1].onmessage?.(new Uint8Array([0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0]).buffer);
    await waitFor(() =>
      expect(container.querySelector('[data-at="0-1"]')?.hasAttribute("data-current")).toBe(true),
    );
    fireEvent.click(screen.getByRole("button", { name: t("workspace:teleprompter.panel.mark") }));
    await waitFor(() => expect(commandMock("teleprompterMark")).toHaveBeenCalledWith({ block: 0, line: 1 }));
    expect(await screen.findByText(t("workspace:teleprompter.panel.markPending", { count: 2 }))).toBeTruthy();
  });

  it("Rust 写回之后告知：次数清零，并说写回了几处", async () => {
    render(withI18n(createElement(TeleprompterPanel)));
    await screen.findByText("你好。");
    act(() => window.__lunaTeleprompterMarks?.(3, 0));
    expect(await screen.findByText(t("workspace:teleprompter.panel.markPending", { count: 3 }))).toBeTruthy();
    act(() => window.__lunaTeleprompterMarks?.(0, 3));
    expect(await screen.findByText(t("workspace:teleprompter.panel.marksWritten", { count: 3 }))).toBeTruthy();
    expect(screen.queryByText(t("workspace:teleprompter.panel.markPending", { count: 3 }))).toBeNull();
  });

  it("录制页签的「开始跟读」：Rust 请面板开跟读；已经开着再请一次不重开", async () => {
    setCommandReply("teleprompterFollowStart", { status: "ok", data: { status: "recording" } });
    render(withI18n(createElement(TeleprompterPanel)));
    await screen.findByText("你好。");
    act(() => window.__lunaTeleprompterFollow?.());
    expect(await screen.findByRole("button", { name: t("workspace:teleprompter.panel.followStop") })).toBeTruthy();
    act(() => window.__lunaTeleprompterFollow?.());
    expect(commandMock("teleprompterFollowStart")).toHaveBeenCalledTimes(1);
  });
});

/** 装了「提词器」应用（扩展可用）、倒计时 3 秒的那一套设置与内容。 */
function extendedSetup() {
  setCommandReply("listSettings", {
    status: "ok",
    data: [
      { key: "teleprompter.follow_enabled", current_value: { kind: "bool", value: true }, control: { kind: "toggle" } },
      { key: "teleprompter.capture_hidden", current_value: { kind: "bool", value: true }, control: { kind: "toggle" } },
      {
        key: "teleprompter.countdown_secs",
        current_value: { kind: "str", value: "3" },
        control: { kind: "select", options: ["0", "3", "5"], style: "segmented" },
      },
    ] as unknown as SettingSpecDto[],
  });
  setCommandReply("teleprompterContent", {
    status: "ok",
    data: {
      path: "Notes/口播.md",
      title: "口播",
      extended: true,
      anchor: null,
      blocks: [
        { heading: null, lines: [{ text: "你好。", marker: null, millis: 500, spans: [{ kind: "text" as const, text: "你好。" }], cue: false }, { text: "今天聊三件事", marker: null, millis: 1500, spans: [{ kind: "text" as const, text: "今天聊三件事" }], cue: false }] },
        { heading: "结尾", lines: [{ text: "再见。", marker: null, millis: 500, spans: [{ kind: "text" as const, text: "再见。" }], cue: false }] },
      ],
    },
  });
  setCommandReply("teleprompterSnapToCamera", { status: "ok", data: null });
}

describe("提词器应用的扩展（ADR-176 ⑤）", () => {
  it("预计剩余 = 从这一行起每行毫秒数之和", () => {
    const content = {
      path: "a",
      title: "a",
      extended: true,
      anchor: null,
      blocks: [
        { heading: null, lines: [{ text: "a", marker: null, millis: 100, spans: [{ kind: "text" as const, text: "a" }], cue: false }, { text: "b", marker: null, millis: 200, spans: [{ kind: "text" as const, text: "b" }], cue: false }] },
        { heading: "h", lines: [{ text: "c", marker: null, millis: 400, spans: [{ kind: "text" as const, text: "c" }], cue: false }] },
      ],
    };
    expect(remainingMillis(content, { block: 0, line: 0 })).toBe(700);
    expect(remainingMillis(content, { block: 0, line: 1 })).toBe(600);
    expect(remainingMillis(content, { block: 1, line: 0 })).toBe(400);
  });

  it("只装创作（没有扩展）：没有计时，菜单里只有跟读开关", async () => {
    render(withI18n(createElement(TeleprompterPanel)));
    await screen.findByText("你好。");
    expect(screen.queryByRole("timer")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: t("workspace:teleprompter.panel.more_menu") }));
    expect(screen.getByRole("menuitemcheckbox", { name: t("workspace:teleprompter.panel.menu.follow") })).toBeTruthy();
    expect(screen.queryByRole("menuitem", { name: t("workspace:teleprompter.panel.menu.camera") })).toBeNull();
  });

  it("开始前先倒计时，倒数期间不滚动、不开跟读；暂停可以取消倒计时", async () => {
    extendedSetup();
    render(withI18n(createElement(TeleprompterPanel)));
    await screen.findByText("你好。");
    expect(await screen.findByRole("timer")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: t("workspace:teleprompter.panel.play") }));
    expect((await screen.findAllByText("3")).length).toBeGreaterThan(0);
    // 倒数中按钮变成「暂停」：点一下就取消。
    fireEvent.click(screen.getByRole("button", { name: t("workspace:teleprompter.panel.pause") }));
    expect(screen.getByRole("button", { name: t("workspace:teleprompter.panel.play") })).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: t("workspace:teleprompter.panel.follow") }));
    expect(commandMock("teleprompterFollowStart")).not.toHaveBeenCalled();
  });

  it("菜单里能贴近镜头、切换「尽量不被录屏」与倒计时档位", async () => {
    extendedSetup();
    render(withI18n(createElement(TeleprompterPanel)));
    await screen.findByText("你好。");
    const open = () => fireEvent.click(screen.getByRole("button", { name: t("workspace:teleprompter.panel.more_menu") }));
    open();
    fireEvent.click(screen.getByRole("menuitem", { name: t("workspace:teleprompter.panel.menu.camera") }));
    await waitFor(() => expect(commandMock("teleprompterSnapToCamera")).toHaveBeenCalledTimes(1));
    open();
    fireEvent.click(screen.getByRole("menuitemcheckbox", { name: t("workspace:teleprompter.panel.menu.captureHidden") }));
    expect(commandMock("setSetting")).toHaveBeenCalledWith("teleprompter.capture_hidden", { kind: "bool", value: false });
    open();
    fireEvent.click(screen.getByRole("menuitemcheckbox", { name: t("workspace:teleprompter.panel.menu.countdownOff") }));
    expect(commandMock("setSetting")).toHaveBeenCalledWith("teleprompter.countdown_secs", { kind: "str", value: "0" });
  });
});

describe("按键与遥控（ADR-176 ⑥）", () => {
  it("换行 / 换段的目标：换段落到相邻一段的第一行，在本段中间往前先回本段第一行", () => {
    const box = document.createElement("div");
    box.innerHTML =
      '<p data-at="0-0"></p><p data-at="0-1"></p><p data-at="1-0"></p><p data-at="1-1"></p><p data-at="2-0"></p>';
    expect(neighborLine(box, { block: 0, line: 1 }, 1, "line")).toEqual({ block: 1, line: 0 });
    expect(neighborLine(box, { block: 0, line: 0 }, -1, "line")).toBeNull();
    expect(neighborLine(box, { block: 0, line: 1 }, 1, "block")).toEqual({ block: 1, line: 0 });
    expect(neighborLine(box, { block: 1, line: 1 }, -1, "block")).toEqual({ block: 1, line: 0 });
    expect(neighborLine(box, { block: 1, line: 0 }, -1, "block")).toEqual({ block: 0, line: 0 });
    expect(neighborLine(box, { block: 2, line: 0 }, 1, "block")).toBeNull();
  });

  it("跟读时按「下一行」与点一行：Rust 从那一行接着跟", async () => {
    setCommandReply("teleprompterFollowStart", { status: "ok", data: { status: "recording" } });
    setCommandReply("teleprompterFollowSeek", undefined);
    const { container } = render(withI18n(createElement(TeleprompterPanel)));
    await screen.findByText("你好。");
    fireEvent.click(screen.getByRole("button", { name: t("workspace:teleprompter.panel.follow") }));
    await screen.findByRole("button", { name: t("workspace:teleprompter.panel.followStop") });
    channels[1].onmessage?.(new Uint8Array([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]).buffer);
    await waitFor(() => expect(container.querySelector('[data-at="0-0"]')?.hasAttribute("data-current")).toBe(true));
    act(() => window.__lunaTeleprompterKey?.("line_down"));
    expect(commandMock("teleprompterFollowSeek")).toHaveBeenLastCalledWith({ block: 0, line: 1 });
    fireEvent.click(screen.getByText("你好。"));
    expect(commandMock("teleprompterFollowSeek")).toHaveBeenLastCalledWith({ block: 0, line: 0 });
  });

  it("按键「加速」步进速度；「收起」收起浮窗", async () => {
    render(withI18n(createElement(TeleprompterPanel)));
    await screen.findByText("你好。");
    act(() => window.__lunaTeleprompterKey?.("faster"));
    expect(commandMock("setSetting")).toHaveBeenCalledWith("teleprompter.speed", { kind: "int", value: 45 });
    act(() => window.__lunaTeleprompterKey?.("close"));
    await waitFor(() => expect(commandMock("teleprompterHide")).toHaveBeenCalledTimes(1));
  });

  it("遥控模式开着时常亮「遥控中」；菜单里能开关", async () => {
    extendedSetup();
    render(withI18n(createElement(TeleprompterPanel)));
    await screen.findByText("你好。");
    expect(screen.queryByText(t("workspace:teleprompter.panel.remoteOn"))).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: t("workspace:teleprompter.panel.more_menu") }));
    fireEvent.click(screen.getByRole("menuitemcheckbox", { name: t("workspace:teleprompter.panel.menu.remote") }));
    expect(commandMock("setSetting")).toHaveBeenCalledWith("teleprompter.remote_enabled", { kind: "bool", value: true });
    expect(await screen.findByText(t("workspace:teleprompter.panel.remoteOn"))).toBeTruthy();
  });
});

describe("口播记号与改稿锚定（ADR-176 ⑦，PRD-20-014 / 015）", () => {
  it("按 Rust 拆好的段画：强调加粗、停顿是一根竖条、注释是小字、动作提示是一条彩色条", async () => {
    setCommandReply("teleprompterContent", {
      status: "ok",
      data: {
        path: "Notes/记号.md",
        title: "记号",
        extended: true,
        anchor: null,
        blocks: [
          {
            heading: null,
            lines: [
              {
                text: "今天聊 三件事",
                marker: null,
                millis: 1600,
                cue: false,
                spans: [
                  { kind: "text" as const, text: "今天聊" },
                  { kind: "pause" as const },
                  { kind: "emphasis" as const, text: "三件事" },
                  { kind: "note" as const, text: "慢一点" },
                ],
              },
              { text: "", marker: null, millis: 0, cue: true, spans: [{ kind: "text" as const, text: "举起产品" }] },
            ],
          },
        ],
      },
    });
    const { container } = render(withI18n(createElement(TeleprompterPanel)));
    expect((await screen.findByText("三件事")).tagName).toBe("STRONG");
    expect(screen.getByRole("img", { name: t("workspace:teleprompter.panel.pauseMark") })).toBeTruthy();
    expect(screen.getByText("慢一点").className).toContain("luna-teleprompter__note");
    expect(container.querySelector(".luna-teleprompter__cue")?.textContent).toBe("举起产品");
  });

  it("同一篇被保存后重读：带上刚才那一行请 Rust 找回，跟读时从找回的那一行接着跟", async () => {
    setCommandReply("teleprompterFollowStart", { status: "ok", data: { status: "recording" } });
    setCommandReply("teleprompterFollowSeek", undefined);
    render(withI18n(createElement(TeleprompterPanel)));
    await screen.findByText("你好。");
    fireEvent.click(screen.getByRole("button", { name: t("workspace:teleprompter.panel.follow") }));
    await screen.findByRole("button", { name: t("workspace:teleprompter.panel.followStop") });
    // 跟读念到第 0 段第 1 行「今天聊三件事」。
    channels[1].onmessage?.(new Uint8Array([0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0]).buffer);
    // 改稿：前面插了一行，那一行挪到第 2 行；Rust 找回后回报新位置。
    setCommandReply("teleprompterContent", {
      status: "ok",
      data: {
        path: "Creator/P/P-脚本.md",
        title: "P-脚本",
        extended: false,
        anchor: { block: 0, line: 2 },
        blocks: [
          {
            heading: "开头",
            lines: [
              { text: "新加的一句。", marker: null, millis: 0, cue: false, spans: [{ kind: "text" as const, text: "新加的一句。" }] },
              { text: "你好。", marker: null, millis: 0, cue: false, spans: [{ kind: "text" as const, text: "你好。" }] },
              { text: "今天聊三件事", marker: null, millis: 0, cue: false, spans: [{ kind: "text" as const, text: "今天聊三件事" }] },
            ],
          },
        ],
      },
    });
    act(() => window.__lunaReloadTeleprompter?.());
    await waitFor(() =>
      expect(commandMock("teleprompterContent")).toHaveBeenLastCalledWith({
        path: "Creator/P/P-脚本.md",
        block: 0,
        line: 1,
        text: "今天聊三件事",
      }),
    );
    await waitFor(() => expect(commandMock("teleprompterFollowSeek")).toHaveBeenLastCalledWith({ block: 0, line: 2 }));
  });
});

