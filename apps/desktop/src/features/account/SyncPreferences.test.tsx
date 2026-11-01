// @vitest-environment happy-dom
/**
 * 「云同步」一节的设备级同步偏好（ADR-156）：按注册表声明的形态渲染（下拉 / 开关），改动写回设置，
 * 设置变了跟着刷新；注册表里不存在（不可用）的键不渲染。
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, waitFor } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import {
  commandMock,
  emitEvent,
  resetBindingsDouble,
  setCommandReply,
} from "../../ipc/bindings-test-double";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { SyncPreferences } from "./SyncPreferences";

function spec(key: string, control: unknown, value: unknown) {
  return {
    key,
    group: "storage",
    scope: "device",
    default_value: value,
    current_value: value,
    modified: false,
    available: true,
    resettable: true,
    requires_restart: false,
    surface: { kind: "panel" },
    visibility: "advanced",
    section: null,
    i18n_key: `settings:${key}`,
    desc_i18n_key: `settings:${key}_desc`,
    desc_presentation: "inline",
    control,
    shortcut_conflict: null,
  };
}

function specs(mode: string, metered: boolean) {
  return [
    spec(
      "sync.recordings_mode",
      { kind: "select", options: ["on_demand", "full", "off"], style: "dropdown" },
      { kind: "str", value: mode },
    ),
    spec("sync.pause_large_on_metered", { kind: "toggle" }, { kind: "bool", value: metered }),
    spec("sync.pause_large_on_battery", { kind: "toggle" }, { kind: "bool", value: true }),
    spec("appearance.theme", { kind: "toggle" }, { kind: "bool", value: true }),
  ];
}

beforeEach(() => resetBindingsDouble());
afterEach(() => cleanup());

describe("SyncPreferences", () => {
  it("按注册表渲染三项、只渲染同步的键，改动写回设置并随设置变化刷新", async () => {
    setCommandReply("listSettings", { status: "ok", data: specs("on_demand", true) });
    setCommandReply("setSetting", { status: "ok", data: null });
    const view = render(withI18n(<SyncPreferences />));

    const select = (await waitFor(() =>
      view.getByRole("combobox", { name: t("settings:sync.recordings_mode") }),
    )) as HTMLSelectElement;
    expect(select.value).toBe("on_demand");
    expect(view.getByText(t("settings:option.sync.recordings_mode.off"))).toBeTruthy();
    expect(view.queryByText(t("settings:appearance.theme"))).toBeNull();

    fireEvent.change(select, { target: { value: "full" } });
    expect(commandMock("setSetting")).toHaveBeenCalledWith("sync.recordings_mode", {
      kind: "str",
      value: "full",
    });

    const metered = view.getByRole("switch", { name: t("settings:sync.pause_large_on_metered") });
    expect(metered.getAttribute("aria-checked")).toBe("true");
    fireEvent.click(metered);
    expect(commandMock("setSetting")).toHaveBeenCalledWith("sync.pause_large_on_metered", {
      kind: "bool",
      value: false,
    });

    setCommandReply("listSettings", { status: "ok", data: specs("full", false) });
    act(() => emitEvent("settingsChanged", { key: "sync.pause_large_on_metered" }));
    await waitFor(() =>
      expect(
        view
          .getByRole("switch", { name: t("settings:sync.pause_large_on_metered") })
          .getAttribute("aria-checked"),
      ).toBe("false"),
    );
  });
});
