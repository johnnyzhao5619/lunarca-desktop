// @vitest-environment happy-dom
/**
 * 数值项的越界由 **Rust 裁决**，不在前端夹回范围内（ADR-003 / PRD-09-070 ④）。
 *
 * 此前 `onBlur` 里一句 `clampToStep(draft, min, max, step)`：用户输的 9999 静默变成 200
 * 落盘——既没人告诉他上限是多少，也让 Rust 那条完整的
 * `OutOfRange → SETTING_RANGE → error.range` 链**永远走不到**（红边与那句判语成了死代码，
 * 真机 2026-09-10 复现）。这份用例钉住"照原样送出去"，它是那条链还活着的唯一前提。
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { resetBindingsDouble } from "../../ipc/bindings-test-double";
import { withI18n } from "../../i18n/i18n-test-fixture";
import { SettingControl } from "./SettingControls";
import { createSettingSpecFixture } from "./settings-test-fixture";

const SPEC = {
  ...createSettingSpecFixture(),
  key: "recording.disk_warn_gib",
  control: {
    kind: "number" as const,
    min: 1,
    max: 200,
    step: 1,
    unit: "gibibyte",
    display_divisor: 1,
    display_unit: "gibibyte",
    style: "stepper",
  },
  current_value: { kind: "int" as const, value: 1 },
  modified: false, available: true, default_value: { kind: "int" as const, value: 1 },
  resettable: true,
};

beforeEach(resetBindingsDouble);
afterEach(cleanup);

describe("数值越界交给 Rust 裁决", () => {
  it("时长按 Rust 显示比例呈现秒，滑块键盘提交仍保存毫秒", () => {
    const written: number[] = [];
    const spec = { ...SPEC, key: "editor.autosave_debounce_ms", i18n_key: "settings:editor.autosave_debounce_ms",
      control: { ...SPEC.control, min: 200, max: 5000, step: 100, unit: "millisecond", style: "slider", display_divisor: 1000, display_unit: "second" },
      current_value: { kind: "int" as const, value: 1500 } };
    const view = render(withI18n(<SettingControl spec={spec} onVaultChanged={() => {}} setValue={async (_key, value) => {
      if (value.kind === "int") written.push(value.value); return value;
    }} />));
    const input = view.container.querySelector("input[type=range]")!;
    expect(view.container.querySelector("output")?.textContent).toBe("1.5 秒");
    expect(input.getAttribute("aria-valuetext")).toBe("1.5 秒");
    fireEvent.change(input, { target: { value: "1900" } }); fireEvent.keyUp(input, { key: "ArrowRight" });
    expect(written).toEqual([1900]);
    expect(view.container.querySelector("output")?.textContent).toBe("1.9 秒");
  });
  it("输入越界值时照原样送出，不在前端夹回范围内", () => {
    const written: number[] = [];
    const view = render(
      withI18n(
        <SettingControl
          spec={SPEC}
          setValue={async (_key, value) => {
            if (value.kind === "int") written.push(value.value);
            return value;
          }}
          onVaultChanged={() => {}}
        />,
      ),
    );
    const input = view.container.querySelector("input[type=number]")!;
    fireEvent.change(input, { target: { value: "9999" } });
    fireEvent.blur(input);
    expect(written, "越界值必须原样送到 Rust——夹回 200 会让那条拒绝链永远走不到").toEqual([
      9999,
    ]);
  });

  it("范围内的值照常写入", () => {
    const written: number[] = [];
    const view = render(
      withI18n(
        <SettingControl
          spec={SPEC}
          setValue={async (_key, value) => {
            if (value.kind === "int") written.push(value.value);
            return value;
          }}
          onVaultChanged={() => {}}
        />,
      ),
    );
    const input = view.container.querySelector("input[type=number]")!;
    fireEvent.change(input, { target: { value: "8" } });
    fireEvent.blur(input);
    expect(written).toEqual([8]);
  });
});
