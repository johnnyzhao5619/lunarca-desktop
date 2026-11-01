// @vitest-environment happy-dom
/**
 * 录快捷键期间先把全局键整体收起来（PRD-09-003）：系统级热键与事件 tap 都不经过 webview，
 * 不收起的话按到已被占用的组合会当场唤起占用者。
 *
 * 这份用例钉两件事：开始录制就收起来，离开录制的每一条出路都装回去。漏一条的代价是全局键
 * 从此失灵到重启。
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble } from "../../ipc/bindings-test-double";
import { withI18n } from "../../i18n/i18n-test-fixture";
import { SettingControl } from "./SettingControls";
import { createSettingSpecFixture } from "./settings-test-fixture";

const SPEC = {
  ...createSettingSpecFixture(),
  key: "shortcuts.voice_input",
  control: { kind: "shortcut" as const, scope: "global", trigger_mode_visibility: "advanced" },
  current_value: { kind: "str" as const, value: "double_tap:cmd_or_ctrl+shift+v" },
};

function mount(spec = SPEC, advancedMode = false) {
  return render(
    withI18n(
      <SettingControl
        spec={spec}
        advancedMode={advancedMode}
        setValue={async (_key, value) => value}
        onVaultChanged={() => {}}
      />,
    ),
  );
}

/** `set_shortcut_capture` 收到过的那串开关。 */
function captureCalls(): boolean[] {
  return commandMock("setShortcutCapture").mock.calls.map((call) => call[0] as boolean);
}

beforeEach(resetBindingsDouble);
afterEach(cleanup);

describe("绑定与未绑定是同一种长相", () => {
  it("简易层不露出触发方式编辑器，完整设置才可编辑", () => {
    const simple = mount();
    expect(simple.queryByRole("radiogroup")).toBeNull();
    expect(simple.getByRole("button")).toBeTruthy();
    cleanup();
    const full = mount(SPEC, true);
    expect(full.getByRole("radiogroup")).toBeTruthy();
    expect(full.getAllByRole("radio")).toHaveLength(2);
  });
  it("两种状态都渲染成键帽——未绑定不是一句斜体灰字", () => {
    // 同一列里两种长相时，用户读到的不是"这一格是空的"，而是"这一行和别的行不是一个
    // 东西"（真机症状 ⑤：「录音打点」是输入框里的「未绑定」，其余是键帽）。
    const bound = mount();
    expect(bound.container.querySelectorAll(".luna-shortcut-recorder kbd").length).toBeGreaterThan(0);
    cleanup();

    const unbound = mount({ ...SPEC, current_value: { kind: "str" as const, value: "" } });
    const empty = unbound.container.querySelector(".luna-shortcut-recorder__empty");
    expect(empty, "未绑定态应有空槽").toBeTruthy();
    expect(empty?.tagName.toLowerCase(), "空槽必须也是 kbd：形状与行高由那一枚原语给").toBe(
      "kbd",
    );
  });
});

describe("录快捷键期间的全局键", () => {
  it("开始录制就收起来", () => {
    const view = mount();
    fireEvent.click(view.container.querySelector(".luna-shortcut-recorder")!);
    expect(captureCalls()).toEqual([true]);
  });

  it("录到一个键之后装回去", () => {
    const view = mount();
    fireEvent.click(view.container.querySelector(".luna-shortcut-recorder")!);
    fireEvent.keyDown(window, { key: "k", code: "KeyK", metaKey: true, shiftKey: true });
    expect(captureCalls()).toEqual([true, false]);
  });

  it("按 Esc 取消也装回去", () => {
    const view = mount();
    fireEvent.click(view.container.querySelector(".luna-shortcut-recorder")!);
    fireEvent.keyDown(window, { key: "Escape", code: "Escape" });
    expect(captureCalls()).toEqual([true, false]);
  });

  it("点到别处、以及整个控件被卸载，同样装回去", () => {
    const view = mount();
    fireEvent.click(view.container.querySelector(".luna-shortcut-recorder")!);
    fireEvent.mouseDown(document.body);
    expect(captureCalls()).toEqual([true, false]);

    // 卸载：用户在录制中途切走了设置页。
    resetBindingsDouble();
    const second = mount();
    fireEvent.click(second.container.querySelector(".luna-shortcut-recorder")!);
    cleanup();
    expect(captureCalls()).toEqual([true, false]);
  });
});
