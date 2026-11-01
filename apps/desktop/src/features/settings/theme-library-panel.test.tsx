// @vitest-environment happy-dom
/**
 * 设置 › 外观的主题选择面（ADR-168 ⑦）：只「选用哪一套」。装上的主题在在线目录里有新版本时（Rust 给的
 * `update_to`）卡片上写「可更新到 …」并说去哪里更新（PRD-18-011：对应选择面也要标出）；这里不放更新按钮。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, within } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { ThemeLibraryPanel } from "./ThemeLibraryPanel";

function entry(id: string, name: string, source: "builtin" | "content", update_to: string | null = null) {
  return {
    id,
    version: "1.2.0",
    name,
    author: "Ada",
    license: "CC0-1.0",
    source,
    css: null,
    swatch_css: `[data-theme-swatch="${id}"]{}`,
    update_to,
  };
}

beforeEach(() => {
  resetBindingsDouble();
  setCommandReply("themeLibrary", {
    in_use: "luna",
    themes: [
      entry("luna", "Lunarca", "builtin"),
      entry("com.example.dusk", "黄昏", "content", "1.3.0"),
      entry("com.example.bay", "海湾", "content"),
    ],
    disabled: [],
    can_check_updates: false,
  });
});
afterEach(cleanup);

describe("设置 › 外观的主题选择面", () => {
  it("有新版本的那套写「可更新到 …」，其余照旧写作者与许可；不放更新按钮", async () => {
    render(withI18n(<ThemeLibraryPanel spec={undefined} setValue={vi.fn()} />));
    const group = within(await screen.findByRole("radiogroup", { name: t("settings:theme_library.title") }));
    expect(
      await group.findByText(
        t("settings:theme_library.update_meta", { author: "Ada", license: "CC0-1.0", version: "1.3.0" }),
      ),
    ).toBeTruthy();
    expect(group.getByText(t("settings:theme_library.installed_meta", { author: "Ada", license: "CC0-1.0" }))).toBeTruthy();
    expect(screen.queryByRole("button", { name: t("workspace:apps.themes.market.update") })).toBeNull();
  });
});
