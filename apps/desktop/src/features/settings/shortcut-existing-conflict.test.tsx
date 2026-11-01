// 早已存在的撞键（W3b）：Rust 给出另一项的 key，设置行把它翻成名字提示出来；没撞就不出现。
import { createElement } from "react";
import { describe, expect, it } from "vitest";
import { SettingField } from "./SettingField";
import { SettingsFieldContext } from "./settings-field-context";
import {
  createSettingSpecFixture,
  createSettingsControllerFixture,
  renderWithSettingsHosts,
  translateForTest,
} from "./settings-test-fixture";

function render(conflict: string | null): string {
  const spec = createSettingSpecFixture({
    key: "shortcuts.toggle_reading_view",
    group: "shortcuts",
    i18n_key: "settings:shortcuts.toggle_reading_view",
    desc_i18n_key: "settings:shortcuts.toggle_reading_view_desc",
    default_value: { kind: "str", value: "cmd_or_ctrl+e" },
    current_value: { kind: "str", value: "cmd_or_ctrl+e" },
    control: { kind: "shortcut", scope: "in_app", trigger_mode_visibility: "advanced" },
    shortcut_conflict: conflict,
  } as Parameters<typeof createSettingSpecFixture>[0]);
  const controller = createSettingsControllerFixture({});
  return renderWithSettingsHosts(
    createElement(
      SettingsFieldContext.Provider,
      { value: controller },
      createElement(SettingField, { spec, setValue: controller.setValue }),
    ),
  );
}

describe("快捷键行的存量撞键提示", () => {
  it("把另一项翻成名字说出来", () => {
    const other = translateForTest("settings:shortcuts.voice_input");
    expect(render("shortcuts.voice_input")).toContain(
      translateForTest("settings:shortcuts.existing_conflict", { other }),
    );
  });

  it("没撞就没有这句", () => {
    expect(render(null)).not.toContain("luna-settings-field__warning");
  });
});
