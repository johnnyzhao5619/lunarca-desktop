// @vitest-environment happy-dom
/**
 * 提词器面板（ADR-162 ⑤ C4a，PRD-19-016）：内容与偏好都来自 Rust，面板只渲染、只发命令。
 */

import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));
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
import { TeleprompterPanel, decodeReadAt, lineAtReadingLine } from "./TeleprompterPanel";

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
    ],
  });
  setCommandReply("setSetting", { status: "ok", data: null });
  setCommandReply("teleprompterHide", undefined);
  setCommandReply("teleprompterContent", {
    status: "ok",
    data: {
      path: "Creator/P/P-脚本.md",
      title: "P-脚本",
      blocks: [
        { heading: "开头", lines: [{ text: "你好。", marker: "口播" }, { text: "今天聊三件事", marker: null }] },
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
    expect(await screen.findByText(t("workspace:creator.teleprompter.empty"))).toBeTruthy();
  });

  it("步进按规格的步长写设置并夹在上下限内；到顶的那一侧不可点", async () => {
    render(withI18n(createElement(TeleprompterPanel)));
    await screen.findByText("你好。");
    const size = t("workspace:creator.teleprompter.fontSize");
    await waitFor(() => expect(screen.getByText(`${size} 118`)).toBeTruthy());
    fireEvent.click(screen.getByRole("button", { name: t("workspace:creator.teleprompter.more", { what: size }) }));
    expect(commandMock("setSetting")).toHaveBeenCalledWith("teleprompter.font_size", { kind: "int", value: 120 });
    expect(
      (screen.getByRole("button", { name: t("workspace:creator.teleprompter.more", { what: size }) }) as HTMLButtonElement)
        .disabled,
    ).toBe(true);
    const speed = t("workspace:creator.teleprompter.speed");
    fireEvent.click(screen.getByRole("button", { name: t("workspace:creator.teleprompter.less", { what: speed }) }));
    expect(commandMock("setSetting")).toHaveBeenCalledWith("teleprompter.speed", { kind: "int", value: 35 });
  });

  it("镜像只翻文字；收起走 Rust 的隐藏命令", async () => {
    const { container } = render(withI18n(createElement(TeleprompterPanel)));
    await screen.findByText("你好。");
    fireEvent.click(screen.getByRole("button", { name: t("workspace:creator.teleprompter.mirror") }));
    expect(container.querySelector(".luna-teleprompter")?.hasAttribute("data-mirror")).toBe(true);
    expect(commandMock("setSetting")).toHaveBeenCalledWith("teleprompter.mirror", { kind: "bool", value: true });
    fireEvent.click(screen.getByRole("button", { name: t("workspace:creator.teleprompter.close") }));
    await waitFor(() => expect(commandMock("teleprompterHide")).toHaveBeenCalledTimes(1));
    // 没在跟读：不去停一个不存在的会话。
    expect(commandMock("recordingStop")).not.toHaveBeenCalled();
  });
});

describe("跟读", () => {
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
    fireEvent.click(screen.getByRole("button", { name: t("workspace:creator.teleprompter.follow") }));
    expect(await screen.findByRole("button", { name: t("workspace:creator.teleprompter.followStop") })).toBeTruthy();
    expect(
      (screen.getByRole("button", { name: t("workspace:creator.teleprompter.play") }) as HTMLButtonElement).disabled,
    ).toBe(true);
    // 第二条 Channel 是位置：Rust 说念到了第 0 段第 1 行。
    channels[1].onmessage?.(new Uint8Array([0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0]).buffer);
    await waitFor(() =>
      expect(container.querySelector('[data-at="0-1"]')?.hasAttribute("data-current")).toBe(true),
    );
    fireEvent.click(screen.getByRole("button", { name: t("workspace:creator.teleprompter.close") }));
    await waitFor(() => expect(commandMock("teleprompterHide")).toHaveBeenCalledTimes(1));
    expect(commandMock("recordingStop")).toHaveBeenCalledTimes(1);
  });

  it("开不起来（麦克风被占等）如实说明，开关复位", async () => {
    setCommandReply("teleprompterFollowStart", { status: "error", error: "MIC_BUSY: x" });
    render(withI18n(createElement(TeleprompterPanel)));
    await screen.findByText("你好。");
    fireEvent.click(screen.getByRole("button", { name: t("workspace:creator.teleprompter.follow") }));
    await waitFor(() => expect(screen.getByRole("button", { name: t("workspace:creator.teleprompter.follow") })).toBeTruthy());
    expect(screen.getAllByRole("status").length).toBeGreaterThan(0);
  });

  it("麦克风被占着：命令照常返回但状态是 mic_busy，开关不打开并说明是谁占着", async () => {
    setCommandReply("teleprompterFollowStart", {
      status: "ok",
      data: { status: "mic_busy", message_key: "workspace:creator.teleprompter.followFailed" },
    });
    render(withI18n(createElement(TeleprompterPanel)));
    await screen.findByText("你好。");
    fireEvent.click(screen.getByRole("button", { name: t("workspace:creator.teleprompter.follow") }));
    expect(await screen.findByText(t("workspace:creator.teleprompter.followFailed"))).toBeTruthy();
    expect(screen.getByRole("button", { name: t("workspace:creator.teleprompter.follow") })).toBeTruthy();
  });

  it("阅读线上的那一行：第一条越过阅读线之前的最后一行", () => {
    const box = document.createElement("div");
    Object.defineProperty(box, "scrollTop", { value: 100 });
    Object.defineProperty(box, "clientHeight", { value: 300 });
    for (const [at, top] of [["0-0", 0], ["0-1", 150], ["1-0", 260]] as const) {
      const line = document.createElement("p");
      line.dataset.at = at;
      Object.defineProperty(line, "offsetTop", { value: top });
      box.append(line);
    }
    // 阅读线在 100 + 300 / 3 = 200：0-1 已过线，1-0 还没到。
    expect(lineAtReadingLine(box)).toEqual({ block: 0, line: 1 });
    expect(lineAtReadingLine(document.createElement("div"))).toBeNull();
  });

  it("「标记重来」把此刻那一行报给 Rust：跟读时是高亮行；攒下的次数显示在按钮上", async () => {
    setCommandReply("teleprompterFollowStart", { status: "ok", data: { status: "recording" } });
    setCommandReply("teleprompterMark", { status: "ok", data: 2 });
    const { container } = render(withI18n(createElement(TeleprompterPanel)));
    await screen.findByText("你好。");
    fireEvent.click(screen.getByRole("button", { name: t("workspace:creator.teleprompter.follow") }));
    await screen.findByRole("button", { name: t("workspace:creator.teleprompter.followStop") });
    channels[1].onmessage?.(new Uint8Array([0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0]).buffer);
    await waitFor(() =>
      expect(container.querySelector('[data-at="0-1"]')?.hasAttribute("data-current")).toBe(true),
    );
    fireEvent.click(screen.getByRole("button", { name: t("workspace:creator.teleprompter.mark") }));
    await waitFor(() => expect(commandMock("teleprompterMark")).toHaveBeenCalledWith({ block: 0, line: 1 }));
    expect(await screen.findByText(t("workspace:creator.teleprompter.markPending", { count: 2 }))).toBeTruthy();
  });

  it("Rust 写回之后告知：次数清零，并说写回了几处", async () => {
    render(withI18n(createElement(TeleprompterPanel)));
    await screen.findByText("你好。");
    act(() => window.__lunaTeleprompterMarks?.(3, 0));
    expect(await screen.findByText(t("workspace:creator.teleprompter.markPending", { count: 3 }))).toBeTruthy();
    act(() => window.__lunaTeleprompterMarks?.(0, 3));
    expect(await screen.findByText(t("workspace:creator.teleprompter.marksWritten", { count: 3 }))).toBeTruthy();
    expect(screen.queryByText(t("workspace:creator.teleprompter.markPending", { count: 3 }))).toBeNull();
  });

  it("录制页签的「开始跟读」：Rust 请面板开跟读；已经开着再请一次不重开", async () => {
    setCommandReply("teleprompterFollowStart", { status: "ok", data: { status: "recording" } });
    render(withI18n(createElement(TeleprompterPanel)));
    await screen.findByText("你好。");
    act(() => window.__lunaTeleprompterFollow?.());
    expect(await screen.findByRole("button", { name: t("workspace:creator.teleprompter.followStop") })).toBeTruthy();
    act(() => window.__lunaTeleprompterFollow?.());
    expect(commandMock("teleprompterFollowStart")).toHaveBeenCalledTimes(1);
  });
});
