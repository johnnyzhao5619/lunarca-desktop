// @vitest-environment happy-dom
/**
 * 设置说明的三种呈现（D1，2026-10-08）：由 Rust 逐键声明，前端穷尽消费。
 * 守的是行为而不是 DOM 形状：label_only 不常显但搜索结果里照样给出说明；disclosure 有一个
 * 可聚焦的入口、展开只改视图；深链把用户带到这一行时说明一并展开。
 */
import { createElement } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { withI18n } from "../../i18n/i18n-test-fixture";
import { ConfirmHost } from "../../components/ConfirmHost";
import { ContextMenuHost } from "../../components/ContextMenuHost";
import type { SettingSpecDto } from "../../ipc/bindings";
import { SettingField } from "./SettingField";
import { SettingsFieldContext } from "./settings-field-context";
import {
  createSettingSpecFixture,
  createSettingsControllerFixture,
  translateForTest,
} from "./settings-test-fixture";

const LANGUAGE = {
  key: "appearance.language",
  group: "general",
  i18n_key: "settings:appearance.language",
  desc_i18n_key: "settings:appearance.language_desc",
  control: { kind: "toggle" },
} as const;

function mount(presentation: SettingSpecDto["desc_presentation"], showPage = false) {
  const spec = createSettingSpecFixture({ ...LANGUAGE, desc_presentation: presentation });
  const controller = createSettingsControllerFixture({});
  return render(
    withI18n(
      createElement(
        ContextMenuHost,
        null,
        createElement(
          ConfirmHost,
          null,
          createElement(
            SettingsFieldContext.Provider,
            { value: controller },
            createElement(SettingField, { spec, setValue: controller.setValue, showPage }),
          ),
        ),
      ),
    ),
  );
}

const DESC = () => translateForTest("settings:appearance.language_desc");
const HELP = () =>
  translateForTest("settings:action.about_setting", {
    name: translateForTest("settings:appearance.language"),
  });

afterEach(cleanup);

describe("设置说明的呈现方式", () => {
  it("inline：说明常显，没有帮助入口", () => {
    mount("inline");
    expect(screen.getByText(DESC())).toBeTruthy();
    expect(screen.queryByRole("button", { name: HELP() })).toBeNull();
  });

  it("label_only：常态不显示说明；搜索结果里照样给出（用户凭它找到这一行）", () => {
    mount("label_only");
    expect(screen.queryByText(DESC())).toBeNull();
    cleanup();
    mount("label_only", true);
    expect(screen.getByText(DESC())).toBeTruthy();
  });

  it("disclosure：可聚焦的入口按需展开，只改视图", () => {
    mount("disclosure");
    expect(screen.queryByText(DESC())).toBeNull();
    const help = screen.getByRole("button", { name: HELP() });
    expect(help.getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(help);
    expect(help.getAttribute("aria-expanded")).toBe("true");
    const desc = screen.getByText(DESC());
    const dialog = screen.getByRole("dialog", { name: HELP() });
    expect(help.getAttribute("aria-controls")).toBe(dialog.id);
    expect(dialog.contains(desc)).toBe(true);
  });

  it("disclosure：深链聚焦到这一行时说明一并展开", () => {
    const view = mount("disclosure");
    const row = view.container.querySelector<HTMLElement>('[data-setting-key="appearance.language"]')!;
    fireEvent.focus(row);
    expect(screen.getByText(DESC())).toBeTruthy();
  });
});
