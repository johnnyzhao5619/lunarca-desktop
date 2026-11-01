// @vitest-environment happy-dom
/**
 * 语音 HUD 的会话落点预设（TDD-07 §2.3 / PRD-02-020 / PRD-07-069）。
 *
 * **这一族用例存在的理由**：HUD 是**预热长驻**窗口——第二次之后的唤起复用同一个已挂载
 * 的 webview，`useEffect(..., [])` 不会再跑。于是"挂载时查一次状态"这种写法在第一次之后
 * 就永远读不到新预设，而 Rust 侧一切正常（`prepare_session` 设了、事件也发了）。真机
 * （2026-08-26）表现：从「添加内容 → 语音速记」进去，入口标注写着「存到「笔记 → 速记」」，
 * HUD 摘要却是「注入光标」——PRD-07-069 明令禁止的"入口标注去向 ≠ 实际落点"。
 *
 * 而这个洞**当时全套自动化都是绿的**：Rust 有 emit、有顺序断言，前端有 typecheck——
 * 唯独没有任何东西检查"发出去的事件有没有人听"。
 *
 * **批次 E 把这条从源码字符串断言换成真实渲染**：`expect(HUD).toContain("events.
 * voiceInputHudOpened.listen")` 证明的是"这行代码还在"。TDD-07 §2.3 却明写着"仅证明 Rust
 * 发出了事件或前端包含订阅字符串**不足以**证明闭环"——闭环是"事件到达后这一屏上的落点
 * 摘要真的变了"。现在事件真的派发、组件真的重渲染、断言比对的是屏幕上的那句话。
 */

import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render, waitFor } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

// HUD 借 Tauri 的窗口 API 做拖动与自动增高；两者都与本文件要证明的事无关，替掉即可。
vi.mock("@tauri-apps/api/core", () => ({
  Channel: class {
    onmessage: ((message: unknown) => void) | null = null;
  },
}));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    setSize: () => Promise.resolve(),
    startDragging: () => Promise.resolve(),
  }),
  LogicalSize: class {
    constructor(
      public width: number,
      public height: number,
    ) {}
  },
}));

import {
  commandMock,
  emitEvent,
  listenerCount,
  resetBindingsDouble,
  setCommandReply,
} from "../../ipc/bindings-test-double";

import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { VoiceInputHud } from "./VoiceInputHud";

/**
 * happy-dom（本机 Node 版本下）不提供 `localStorage`，而 HUD 的说话风格记在那里
 * （`usePersistentVoiceMode`）。给一个内存实现即可——本文件不断言它，只是让组件挂得起来。
 */
if (typeof globalThis.localStorage === "undefined") {
  const store = new Map<string, string>();
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: {
      getItem: (key: string) => store.get(key) ?? null,
      setItem: (key: string, value: string) => void store.set(key, String(value)),
      removeItem: (key: string) => void store.delete(key),
      clear: () => store.clear(),
    },
  });
}

/** 一份 idle 会话状态；`landing_mode` 是本文件唯一在意的字段。 */
function voiceState(landingMode: "inject" | "clipboard" | "quick_note") {
  return {
    status: "idle",
    landing_mode: landingMode,
    input_mode: "hold",
    auto_refine: true,
    translate_enabled: false,
    translate_target_lang: "en",
    message_key: "voice:status.idle",
    message_detail: null,
  };
}

/** 屏幕上那句落点文案。 */
function landingLabel(mode: "inject" | "clipboard" | "quick_note"): string {
  return t(`voice-input:landing.${mode}`);
}

const LANDING_MODES = ["inject", "clipboard", "quick_note"] as const;

/**
 * 用户此刻在 HUD 上读到的落点。
 *
 * 判据是空闲胶囊里那枚**落点胶囊**——它就是入口承诺与实际落点对不上的时候，用户唯一
 * 能看见的地方（真机那次截图上写着「注入光标」的正是此前的配置摘要）。返回 `null` 表示
 * 胶囊里同时出现或一个都没出现三档中的某一档，那本身就是缺陷。
 */
function activeLanding(container: HTMLElement): string | null {
  const summary = container.querySelector(".luna-voice__destination")?.textContent ?? "";
  const hits = LANDING_MODES.filter((mode) => summary.includes(landingLabel(mode)));
  return hits.length === 1 ? hits[0] : null;
}

beforeEach(() => {
  resetBindingsDouble();
  setCommandReply("listSettings", { status: "ok", data: [] });
  setCommandReply("modelManagementSnapshot", {
    status: "ok",
    data: { models: [], assignments: [], storage: null, services: [], remote_compute: null },
  });
});
afterEach(cleanup);

async function mountHud(initial: "inject" | "clipboard" | "quick_note") {
  setCommandReply("voiceInputState", voiceState(initial));
  const view = render(withI18n(createElement(VoiceInputHud)));
  await waitFor(() => expect(activeLanding(view.container)).toBe(initial));
  return view;
}

describe("HUD 每次唤起都重读会话状态", () => {
  it("挂载时读一次 Rust 的会话状态，落点照它渲染", async () => {
    const view = await mountHud("inject");
    expect(commandMock("voiceInputState")).toHaveBeenCalledTimes(1);
    expect(activeLanding(view.container)).toBe("inject");
  });

  it("收到 `voice-input://hud-opened` 后重查，屏幕上的落点随之改变", async () => {
    // 这就是那个真机缺陷的最短复现：窗口没有重新挂载，只有一条空载荷事件到达。
    const view = await mountHud("inject");
    setCommandReply("voiceInputState", voiceState("quick_note"));

    await act(async () => {
      emitEvent("voiceInputHudOpened", null);
      await Promise.resolve();
    });

    await waitFor(() =>
      expect(activeLanding(view.container)).toBe("quick_note"),
    );
    expect(commandMock("voiceInputState")).toHaveBeenCalledTimes(2);
  });

  it("落点读的是**会话状态**，不是前端按设置另算一份", async () => {
    // 前端若按设置项自行推断落点，Rust 的会话级预设就再也覆盖不了它（ADR-003）。
    // 判据：设置里没有 `voice_input.landing_mode` 这一项，屏幕照样说得出 Rust 给的落点。
    const view = await mountHud("quick_note");
    expect(activeLanding(view.container)).toBe("quick_note");
  });

  it("订阅在卸载时解掉——预热窗口活得比任何一次会话都久", async () => {
    // 长驻窗口上漏解订阅，重挂载一次就多一个监听，状态查询随之翻倍。
    await mountHud("inject");
    expect(listenerCount("voiceInputHudOpened")).toBe(1);
    cleanup();
    await waitFor(() => expect(listenerCount("voiceInputHudOpened")).toBe(0));

    const before = commandMock("voiceInputState").mock.calls.length;
    emitEvent("voiceInputHudOpened", null);
    await Promise.resolve();
    expect(commandMock("voiceInputState").mock.calls.length).toBe(before);
  });
});
