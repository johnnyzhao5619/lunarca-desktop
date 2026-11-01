import { createElement } from "react";
import { describe, expect, it } from "vitest";
import { PanelSettingRow } from "./PanelSettingRow";
import { SettingsFieldContext } from "./settings-field-context";
import { createSettingSpecFixture, createSettingsControllerFixture, renderWithSettingsHosts, translateForTest } from "./settings-test-fixture";

describe("面板设置行的共享反馈", () => {
  const spec = createSettingSpecFixture({ modified: true });
  function render(overrides: Parameters<typeof createSettingsControllerFixture>[0]) {
    const controller = createSettingsControllerFixture(overrides);
    return renderWithSettingsHosts(createElement(SettingsFieldContext.Provider, { value: controller }, createElement(PanelSettingRow, { spec, setValue: controller.setValue })));
  }
  it("面板行显示宿主的写入错误", () => {
    const html = render({ errors: { [spec.key]: { message: "SETTING_RANGE: value rejected", attempted: { kind: "bool", value: true } } } });
    expect(html).toContain('role="alert"');
    expect(html).toContain('aria-invalid="true"');
  });
  it("保存回执替代同一动作槽的恢复默认按钮", () => {
    const saved = render({ savedKeys: { [spec.key]: true } });
    expect(saved).toContain('role="status"');
    expect(saved).not.toContain(`aria-label="${translateForTest("settings:action.reset")}"`);
    const modified = render({});
    expect(modified).toContain(`aria-label="${translateForTest("settings:action.reset")}"`);
  });
});
