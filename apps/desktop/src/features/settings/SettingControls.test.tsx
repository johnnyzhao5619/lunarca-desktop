/**
 * 设置控件形态守护：控件形态由注册表声明，前端不自行推断。
 */

import { createElement } from "react";
import { describe, expect, it } from "vitest";

import zh from "../../i18n/locales/zh-CN/settings.json";
import { SettingControl } from "./SettingControls";
import { createSettingSpecFixture, renderWithSettingsHosts } from "./settings-test-fixture";

/**
 * 枚举控件的形态由注册表 `control.style` 决定（`SelectStyle`），前端不按文案长度猜——
 * 同一项的中英文标签长度不同，按字数猜会让同一个设置在两种语言下长成两种控件。
 */
describe("枚举控件形态随注册表声明", () => {
  function selectSpec(style: string) {
    return {
      ...createSettingSpecFixture(),
      key: "appearance.theme",
      i18n_key: "settings:appearance.theme",
      desc_i18n_key: "settings:appearance.theme_desc",
      current_value: { kind: "str" as const, value: "dark" },
      modified: false, available: true, default_value: { kind: "str" as const, value: "system" },
    resettable: true,
      control: { kind: "select" as const, options: ["system", "light", "dark"], style },
    };
  }

  it("`segmented` 渲染为 radiogroup，当前档 aria-checked，且组有可读名字", () => {
    const html = renderWithSettingsHosts(
      createElement(SettingControl, {
        spec: selectSpec("segmented"),
        setValue: async (_key, value) => value,
        onVaultChanged: () => {},
      }),
    );
    expect(html).toContain('role="radiogroup"');
    expect(html).toContain(`aria-label="${zh.appearance.theme}"`);
    expect(html).toContain(`aria-checked="true"`);
    expect(html).toContain(zh.option.appearance.theme.dark);
    expect(html).not.toContain("<select");
  });

  it("`dropdown` 仍是原生下拉", () => {
    const html = renderWithSettingsHosts(
      createElement(SettingControl, {
        spec: selectSpec("dropdown"),
        setValue: async (_key, value) => value,
        onVaultChanged: () => {},
      }),
    );
    expect(html).toContain("<select");
    expect(html).not.toContain('role="radiogroup"');
  });
});

/**
 * 有序开关列表的下限由注册表下发（`min_visible`，创作看板至少一列，ADR-162 ⑨）：只剩下限那
 * 几项显示时，它们的开关关不掉；收起的项照样能打开。前端不自己写一份「至少一列」。
 */
describe("有序开关列表遵守注册表下发的显示下限", () => {
  function boardSpec(tokens: string[]) {
    return {
      ...createSettingSpecFixture(),
      key: "creator.board_stages",
      i18n_key: "settings:creator.board_stages",
      desc_i18n_key: "settings:creator.board_stages_desc",
      current_value: { kind: "string_list" as const, value: tokens },
      default_value: { kind: "string_list" as const, value: ["idea", "writing"] },
      modified: true,
      available: true,
      resettable: true,
      control: {
        kind: "ordered_toggle_list" as const,
        options: [
          { id: "idea", label_key: "workspace:creator.stage.idea", locked: false, default_visible: true },
          { id: "writing", label_key: "workspace:creator.stage.writing", locked: false, default_visible: true },
        ],
        hidden_prefix: "hidden:",
        min_visible: 1,
      },
    };
  }

  function switches(html: string): string[] {
    return html.match(/<button[^>]*role="switch"[^>]*>/g) ?? [];
  }

  it("只剩一列显示时，那一列的开关不可关；收起的那列仍可打开", () => {
    const html = renderWithSettingsHosts(
      createElement(SettingControl, {
        spec: boardSpec(["idea", "hidden:writing"]),
        setValue: async (_key, value) => value,
        onVaultChanged: () => {},
      }),
    );
    const [idea, writing] = switches(html);
    expect(idea).toContain('aria-checked="true"');
    expect(idea).toContain("disabled");
    expect(writing).toContain('aria-checked="false"');
    expect(writing).not.toContain("disabled");
  });

  it("两列都显示时都能关", () => {
    const html = renderWithSettingsHosts(
      createElement(SettingControl, {
        spec: boardSpec(["idea", "writing"]),
        setValue: async (_key, value) => value,
        onVaultChanged: () => {},
      }),
    );
    expect(switches(html).every((tag) => !tag.includes("disabled"))).toBe(true);
  });
});
