// @vitest-environment happy-dom
/**
 * 设置导入先预览、确认后才写（C46，2026-10-08）；导出文件名模板的实时预览（C47）。
 * 两件都只消费 Rust：改动清单与示例文件名都不在前端推算。
 */
import { createElement } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { SettingsBackupPanel } from "./SettingsBackupPanel";
import { SettingControl } from "./SettingControls";
import { createSettingSpecFixture } from "./settings-test-fixture";

const ok = (data: unknown) => ({ status: "ok", data });
const flush = () => act(async () => {});

afterEach(() => {
  cleanup();
  resetBindingsDouble();
});

const PREVIEW = {
  path: "/tmp/lunarca-settings.toml",
  changes: [
    { key: "appearance.theme", i18n_key: "settings:appearance.theme", scope: "device" },
    { key: "vault.delete_mode", i18n_key: "settings:vault.delete_mode", scope: "vault" },
  ],
  unchanged: 5,
};

describe("设置导入预览", () => {
  it("选文件后只列出会改变的项，确认时把看过的清单交回核对", async () => {
    setCommandReply("previewSettingsImport", ok(PREVIEW));
    setCommandReply("importSettings", ok({ device: 6, vault: 1 }));
    render(withI18n(createElement(SettingsBackupPanel)));

    fireEvent.click(screen.getByRole("button", { name: t("settings:settings_io.import") }));
    await flush();
    expect(commandMock("importSettings")).not.toHaveBeenCalled();
    expect(screen.getByText(t("settings:appearance.theme"))).toBeTruthy();
    expect(screen.getByText(t("settings:vault.delete_mode"))).toBeTruthy();

    fireEvent.click(
      screen.getByRole("button", { name: t("settings:settings_io.confirm_import", { count: 2 }) }),
    );
    await flush();
    expect(commandMock("importSettings")).toHaveBeenCalledWith(PREVIEW.path, [
      "appearance.theme",
      "vault.delete_mode",
    ]);
  });

  it("两步之间文件变了：如实说没写入，并要求重新查看", async () => {
    setCommandReply("previewSettingsImport", ok(PREVIEW));
    setCommandReply("importSettings", { status: "error", error: "SETTINGS_IMPORT_PLAN_CHANGED" });
    render(withI18n(createElement(SettingsBackupPanel)));
    fireEvent.click(screen.getByRole("button", { name: t("settings:settings_io.import") }));
    await flush();
    fireEvent.click(
      screen.getByRole("button", { name: t("settings:settings_io.confirm_import", { count: 2 }) }),
    );
    await flush();
    expect(screen.getByText(t("settings:settings_io.plan_changed"))).toBeTruthy();
  });
});

describe("导出文件名实时预览", () => {
  it("注册表声明了预览：边输入边显示 Rust 渲染的示例文件名", async () => {
    setCommandReply("previewExportFilename", "会议记录 2026-10-08.srt");
    const spec = createSettingSpecFixture({
      key: "export.filename_template",
      i18n_key: "settings:export.filename_template",
      desc_i18n_key: "settings:export.filename_template_desc",
      default_value: { kind: "str", value: "{name}" },
      current_value: { kind: "str", value: "{name} {date}" },
      control: { kind: "text", preview: "export_filename", multiline: false },
    });
    render(
      withI18n(
        createElement(SettingControl, {
          spec,
          setValue: async (_key, value) => value,
          onVaultChanged: () => {},
        }),
      ),
    );
    await flush();
    expect(commandMock("previewExportFilename")).toHaveBeenCalledWith(
      "{name} {date}",
      t("settings:export.preview_sample_name"),
    );
    expect(screen.getByText(t("settings:export.preview", { name: "会议记录 2026-10-08.srt" }))).toBeTruthy();
  });
});
