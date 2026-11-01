// @vitest-environment happy-dom
/**
 * 创建动作的**唯一注册表**及其两个呈现面（ADR-087 / PRD-07-067/068/069 / TDD-07 §2.2）。
 *
 * 概览此前自持一份私有 `QUICK_ACTIONS`，于是"有哪些添加方式"在应用里有两个答案——而它们
 * 从第一天起就不一样：那份表里有实时字幕和翻译（它们根本不创建任何东西，按 PRD-07-003a
 * 归顶栏工具区），却没有导入资料。这一族用例就是钉住"只剩一份"。
 *
 * **呈现面从源码字符串断言改成真实渲染**（批次 E）：`expect(SOURCE).toContain('role="menu"')`
 * 证明的是"这个字符串还在"，而 PRD-07-067/068 要的是"方向键真的跳过了不可用项"、"过时
 * 快照真的没有覆盖新的"。前者换个等价写法就误红，后者它一辈子也看不见。现在这两件事在
 * happy-dom 里真的发生：按键真的派发、事件真的到达、按钮真的被 `disabled`。
 *
 * DOM 环境的作用域只到本文件（顶部 docblock），不进 `vite.config.ts` 的全局 `environment`。
 */

import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, waitFor } from "@testing-library/react";

import zhWorkspace from "../../i18n/locales/zh-CN/workspace.json";
import enWorkspace from "../../i18n/locales/en-US/workspace.json";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import {
  commandMock,
  emitEvent,
  resetBindingsDouble,
  setCommandReply,
} from "../../ipc/bindings-test-double";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { AddContentPopover } from "./AddContentPopover";
import { CREATE_ACTIONS, CREATE_ACTION_REASON_KEYS } from "./create-actions";
import { NAVIGATE_EVENT } from "../../lib/navigate";

/** 与 Rust `luna_config::CREATE_ACTION_IDS` 逐字相同（那份是唯一词表，这里只是对照）。 */
const RUST_IDS = [
  "text_quick_note",
  "voice_quick_note",
  "new_note",
  "start_recording",
  "import_material",
] as const;

/** 与 Rust `CreateActionReasonDto` 逐字相同。 */
const RUST_REASONS = [
  "vault_unavailable",
  "microphone_permission_required",
  "microphone_permission_denied",
  "audio_busy",
  "voice_session_busy",
  "recording_in_progress",
  "asr_unavailable",
  "service_consent_required",
  "unsupported_platform",
] as const;

function lookup(bundle: unknown, key: string): unknown {
  return key.split(".").reduce<unknown>((node, part) => {
    return typeof node === "object" && node !== null
      ? (node as Record<string, unknown>)[part]
      : undefined;
  }, bundle);
}

describe("创建动作注册表", () => {
  it("恰好覆盖 Rust 词表里的五个 id，不多不少", () => {
    // 类型系统保证不缺（`Record<CreateActionIdDto, …>`），这条守护管的是**多出来的**
    // ——前端偷偷加一个 Rust 不认识的动作，它在「添加内容」里永远不会出现（快照按 Rust
    // 的清单渲染），但在「快速开始」里会被当成合法配置项。
    expect(Object.keys(CREATE_ACTIONS).sort()).toEqual([...RUST_IDS].sort());
  });

  it("每个动作的图标都不一样——两个麦克风图标等于没有图标（PRD-07-069）", () => {
    // 图标是 React 元素；`type` 是那个组件函数本身，直接拿它作身份（`name` 在
    // 压缩/包装后未必稳定，而**同一个组件对象**才是"复用了同一枚图标"的准确判据）。
    const icons = Object.values(CREATE_ACTIONS).map(
      (spec) => (spec.icon as { type?: unknown }).type,
    );
    expect(icons.every(Boolean), "图标必须是元素").toBe(true);
    expect(new Set(icons).size).toBe(icons.length);
  });

  it("每个动作都标注产物去向，且两个速记去同一处、两个资料各去各处", () => {
    const dest = (id: (typeof RUST_IDS)[number]) => CREATE_ACTIONS[id].destinationKey;
    expect(dest("text_quick_note")).toBe(dest("voice_quick_note"));
    expect(dest("new_note")).not.toBe(dest("text_quick_note"));
    expect(dest("start_recording")).not.toBe(dest("import_material"));
    for (const id of RUST_IDS) {
      expect(dest(id).length).toBeGreaterThan(0);
    }
  });

  it("每个动作只调它自己那一条上下文能力（不在注册表里做业务）", async () => {
    for (const id of RUST_IDS) {
      const called: string[] = [];
      const hit = async (name: string): Promise<"arrived"> => {
        called.push(name);
        return "arrived";
      };
      await CREATE_ACTIONS[id].run({
        captureText: () => hit("captureText"),
        captureVoice: () => hit("captureVoice"),
        createNote: () => hit("createNote"),
        startRecording: () => hit("startRecording"),
        importMaterial: () => hit("importMaterial"),
      });
      expect(called.length, `${id} 应恰好触发一条动线`).toBe(1);
    }
  });

  it("终态原样回传——它是「入口到达率」的分子分母，注册表不得替调用方判定", () => {
    // 第一版的 `run` 返回 `void`，于是两个呈现面只能一律记成 `arrived`——那让到达率
    // **只可能是 100%**，一个恒为 1 的比率比没有这条指标更糟。
    const outcomes = ["arrived", "cancelled", "rejected", "failed"] as const;
    return Promise.all(
      outcomes.map(async (outcome) => {
        const stub = async () => outcome;
        const got = await CREATE_ACTIONS.import_material.run({
          captureText: stub,
          captureVoice: stub,
          createNote: stub,
          startRecording: stub,
          importMaterial: stub,
        });
        expect(got).toBe(outcome);
      }),
    );
  });

  it("语音速记与开始录音是两条不同的动线（否则入口标注就是假的）", async () => {
    const trace = async (id: (typeof RUST_IDS)[number]) => {
      let hit = "";
      const mark = async (name: string): Promise<"arrived"> => {
        hit = name;
        return "arrived";
      };
      await CREATE_ACTIONS[id].run({
        captureText: () => mark("captureText"),
        captureVoice: () => mark("captureVoice"),
        createNote: () => mark("createNote"),
        startRecording: () => mark("startRecording"),
        importMaterial: () => mark("importMaterial"),
      });
      return hit;
    };
    expect(await trace("voice_quick_note")).toBe("captureVoice");
    expect(await trace("start_recording")).toBe("startRecording");
  });

  it("全部文案（动作名 / 去向 / 八条原因）在中英双语都在场", () => {
    const keys = [
      ...Object.values(CREATE_ACTIONS).flatMap((spec) => [spec.labelKey, spec.destinationKey]),
      ...Object.values(CREATE_ACTION_REASON_KEYS),
      "create.title",
      "create.repair",
    ];
    for (const bundle of [zhWorkspace, enWorkspace]) {
      for (const key of keys) {
        expect(typeof lookup(bundle, key), `缺文案：${key}`).toBe("string");
      }
    }
  });

  it("八条原因一条不漏——少一格的运行时表现是一个不给任何理由的灰按钮", () => {
    expect(Object.keys(CREATE_ACTION_REASON_KEYS).sort()).toEqual([...RUST_REASONS].sort());
  });
});

// ── 呈现面：真实渲染 ────────────────────────────────────────────────────────

/** 一份快照：默认全可用，`blocked` 里的 id 换成不可用（带原因与修复）。 */
function snapshot(
  revision: number,
  blocked: Partial<Record<(typeof RUST_IDS)[number], unknown>> = {},
) {
  return {
    revision,
    actions: RUST_IDS.map((id) => ({
      id,
      state: blocked[id] ?? { kind: "available" },
    })),
  };
}

const NEEDS_MIC = {
  kind: "needs_attention",
  reason: "microphone_permission_required",
  repair: { kind: "request_microphone_permission" },
};

const NEEDS_VAULT = {
  kind: "needs_attention",
  reason: "vault_unavailable",
  repair: { kind: "open_settings", route: "settings://notes/storage.vault_root" },
};

/** 五条能力桩：记录被调到的那一条，并如实回一个终态。 */
function stubContext(trace: string[]) {
  const hit = (name: string) => async () => {
    trace.push(name);
    return "arrived" as const;
  };
  return {
    captureText: hit("captureText"),
    captureVoice: hit("captureVoice"),
    createNote: hit("createNote"),
    startRecording: hit("startRecording"),
    importMaterial: hit("importMaterial"),
  };
}

afterEach(cleanup);
beforeEach(resetBindingsDouble);

describe("「添加内容」Popover 的真实交互（PRD-07-067 ② / TDD-07 §2.2）", () => {
  async function open(
    initial = snapshot(1),
    onClose: () => void = () => {},
    trace: string[] = [],
    creatorEnabled = true,
  ) {
    setCommandReply("createActionStates", { status: "ok", data: initial });
    // 「创作」没启用时多一行它的介绍（ADR-165 S6b）；其余用例按已启用算，只看五类动作。
    setCommandReply("appsList", [
      {
        id: "creator",
        enabled: creatorEnabled,
        space: "creator",
        note_types: ["video-project", "script", "swipe"],
        permissions: [],
        contributions: [],
      },
    ]);
    const view = render(
      withI18n(createElement(AddContentPopover, { onClose, context: stubContext(trace) })),
    );
    // 快照未到时整块不渲染（一个空的带边框浮层比"晚一帧出现"更糟）。
    await waitFor(() => expect(view.container.querySelector('[role="menu"]')).not.toBeNull());
    return view;
  }

  it("它是一列 `menuitem` 而不是 `dialog`（TDD-07 §4.2c）", async () => {
    const view = await open();
    const menu = view.container.querySelector('[role="menu"]');
    expect(menu?.getAttribute("aria-label")).toBe(t("workspace:create.title"));
    expect(view.container.querySelectorAll('[role="menuitem"]')).toHaveLength(RUST_IDS.length);
    expect(view.container.querySelector('[role="dialog"]'), "菜单不是对话框").toBeNull();
  });

  it("「创作」没启用时多一行它的介绍，点了去「应用」里它的详情页并关闭浮层", async () => {
    const routes: string[] = [];
    const listener = (event: Event) => {
      routes.push(String((event as CustomEvent).detail));
      event.preventDefault();
    };
    window.addEventListener("luna:navigate", listener);
    const onClose = vi.fn();
    const view = await open(snapshot(1), onClose, [], false);
    const intro = await waitFor(() => {
      const found = Array.from(view.container.querySelectorAll('[role="menuitem"]')).find((item) =>
        item.textContent?.includes(t("workspace:create.creatorIntro.label")),
      );
      expect(found).toBeTruthy();
      return found as HTMLElement;
    });
    fireEvent.click(intro);
    window.removeEventListener("luna:navigate", listener);
    expect(routes).toEqual(["apps://creator"]);
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("五类动作恒在场，暂时执行不了的照样列出来并说清原因", async () => {
    const view = await open(snapshot(1, { voice_quick_note: NEEDS_MIC }));
    const items = [...view.container.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')];
    expect(items).toHaveLength(5);
    expect(items[1].disabled, "语音速记此刻不可用").toBe(true);
    expect(view.container.textContent).toContain(
      t(`workspace:${CREATE_ACTION_REASON_KEYS.microphone_permission_required}`),
    );
    // 修复钮可独立聚焦：读屏用户要能单独走到"去授权"上，而不是只听见一句禁用原因。
    const repair = view.getByText(t("workspace:create.repair"));
    expect(repair.tagName).toBe("BUTTON");
    expect((repair as HTMLButtonElement).disabled).toBe(false);
  });

  it("每一行都同时给出动作名与产物去向（PRD-07-069）", async () => {
    const view = await open();
    for (const id of RUST_IDS) {
      expect(view.container.textContent).toContain(t(`workspace:${CREATE_ACTIONS[id].labelKey}`));
      expect(view.container.textContent).toContain(
        t(`workspace:${CREATE_ACTIONS[id].destinationKey}`),
      );
    }
  });

  it("方向键在动作间走，并**跳过**不可用项（首尾相接）", async () => {
    // 焦点停在一个按不动的动作上，用户只会以为方向键坏了。
    const view = await open(snapshot(1, { voice_quick_note: NEEDS_MIC }));
    const items = [...view.container.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')];
    const enabled = items.filter((item) => !item.disabled);
    expect(document.activeElement, "打开即落在第一个可用动作上").toBe(enabled[0]);

    const menu = view.container.querySelector('[role="menu"]')!;
    fireEvent.keyDown(menu, { key: "ArrowDown" });
    expect(document.activeElement, "语音速记被跳过").toBe(enabled[1]);

    enabled[enabled.length - 1].focus();
    fireEvent.keyDown(menu, { key: "ArrowDown" });
    expect(document.activeElement, "走到底再按一下回到开头").toBe(enabled[0]);

    fireEvent.keyDown(menu, { key: "ArrowUp" });
    expect(document.activeElement).toBe(enabled[enabled.length - 1]);
  });

  it("按非模态处理：点外面能关、Esc 能关、Tab 走得出去", async () => {
    // 按模态处理的表现是：点侧栏、点活动栏、点任何地方它都不关，而 Tab 永远在这五行里打转。
    const onClose = vi.fn();
    const view = await open(snapshot(1), onClose);
    const outside = document.createElement("button");
    document.body.append(outside);

    const tabPrevented = !fireEvent.keyDown(document, { key: "Tab" });
    expect(tabPrevented, "非模态不圈焦点").toBe(false);

    fireEvent.pointerDown(view.container.querySelector('[role="menuitem"]')!);
    expect(onClose, "点自己身上不关").not.toHaveBeenCalled();

    fireEvent.pointerDown(outside);
    expect(onClose).toHaveBeenCalledTimes(1);

    fireEvent.keyDown(document, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(2);
    outside.remove();
  });

  it("点一个动作 → 调它那一条能力并关闭浮层", async () => {
    const trace: string[] = [];
    const onClose = vi.fn();
    const view = await open(snapshot(1), onClose, trace);
    fireEvent.click(view.container.querySelectorAll('[role="menuitem"]')[2]);
    expect(trace).toEqual(["createNote"]);
    expect(onClose).toHaveBeenCalled();
  });

  it("修复动作走共享分发：打开设置带走用户，申请权限把他留在原地", async () => {
    const onClose = vi.fn();
    const view = await open(snapshot(1, { voice_quick_note: NEEDS_MIC }), onClose);
    fireEvent.click(view.getByText(t("workspace:create.repair")));
    expect(commandMock("requestPermission")).toHaveBeenCalledWith("microphone");
    // 只弹了一个系统授权框，用户还在原地、马上要重试——收起来等于逼他再打开一次。
    expect(onClose, "申请权限不该收起浮层").not.toHaveBeenCalled();

    cleanup();
    onClose.mockClear();
    const other = await open(snapshot(1, { text_quick_note: NEEDS_VAULT }), onClose);
    let navigatedTo: string | null = null;
    const listen = (event: Event) => {
      navigatedTo = (event as CustomEvent<string>).detail;
    };
    window.addEventListener(NAVIGATE_EVENT, listen);
    fireEvent.click(other.getByText(t("workspace:create.repair")));
    window.removeEventListener(NAVIGATE_EVENT, listen);
    // route 的 wire 值就是深链本体——原样交出去，不拼 URL。
    // 断言的是**去了哪儿**，不是走的哪条通道：通道选择只有一处实现（`lib/navigate.ts`），
    // 也只在那里有一条用例。
    expect(navigatedTo).toBe("settings://notes/storage.vault_root");
    expect(onClose, "被带去设置了，浮层该收起").toHaveBeenCalled();
  });

  it("先订阅后查询：两者之间发生的那次变化不会漏掉", async () => {
    const view = await open();
    // 订阅在查询之前完成，否则这条事件根本没有人收。
    act(() => emitEvent("createActionsStatesChanged", snapshot(2, { new_note: NEEDS_VAULT })));
    await waitFor(() =>
      expect(
        view.container.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')[2].disabled,
      ).toBe(true),
    );
  });

  it("过时快照被丢弃——否则刚被授权的动作会重新变灰", async () => {
    const view = await open();
    act(() => emitEvent("createActionsStatesChanged", snapshot(5, { new_note: NEEDS_VAULT })));
    await waitFor(() =>
      expect(
        view.container.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')[2].disabled,
      ).toBe(true),
    );
    // 慢的那次查询回来了，但它比当前 revision 旧。
    act(() => emitEvent("createActionsStatesChanged", snapshot(3)));
    expect(
      view.container.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')[2].disabled,
      "旧快照覆盖了新的",
    ).toBe(true);
  });

  it("窗口重新激活时重查一次（OS 权限没有可订阅的通道）", async () => {
    // 用户切到系统设置里把麦克风打开再切回来——那是一次收不到任何事件的状态变化。
    await open();
    const before = commandMock("createActionStates").mock.calls.length;
    act(() => void window.dispatchEvent(new Event("focus")));
    await waitFor(() =>
      expect(commandMock("createActionStates").mock.calls.length).toBeGreaterThan(before),
    );
  });

  it("卸载后不再重查——长驻窗口上漏解订阅会让查询逐次翻倍", async () => {
    await open();
    cleanup();
    const after = commandMock("createActionStates").mock.calls.length;
    window.dispatchEvent(new Event("focus"));
    await Promise.resolve();
    expect(commandMock("createActionStates").mock.calls.length).toBe(after);
  });
});
