// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render } from "@testing-library/react";
vi.mock("../ipc/bindings", () => import("../ipc/bindings-test-double"));
import { commandMock, resetBindingsDouble } from "../ipc/bindings-test-double";
import type { SettingSpecDto, SettingValueDto } from "../ipc/bindings";
import { withI18n } from "../i18n/i18n-test-fixture";
import { createSettingSpecFixture } from "../features/settings/settings-test-fixture";
import { CaptionSettingsPreview } from "../features/caption/CaptionSettingsPreview";

function snapshot(overrides: Record<string, SettingValueDto> = {}): SettingSpecDto[] {
  const values: Record<string, SettingValueDto> = {
    "caption.translation_enabled": { kind: "bool", value: true },
    "caption.separate_translation": { kind: "bool", value: false },
    "caption.show_source": { kind: "bool", value: true },
    "caption.translation_first": { kind: "bool", value: false },
    "caption.font_size": { kind: "str", value: "large" },
    "caption.text_color": { kind: "str", value: "white" },
    "caption.text_opacity": { kind: "int", value: 55 },
    "caption.frame_opacity": { kind: "int", value: 60 },
    ...overrides,
  };
  return Object.entries(values).map(([key, current_value]) => ({ ...createSettingSpecFixture(), key, current_value }));
}
beforeEach(resetBindingsDouble);
afterEach(cleanup);

describe("字幕设置离线示例", () => {
  it("挂载、展开及更新快照均不读取或修改设置、不启动采集", () => {
    const view = render(withI18n(<CaptionSettingsPreview specs={snapshot()} />));
    fireEvent.click(view.container.querySelector("summary")!);
    view.rerender(withI18n(<CaptionSettingsPreview specs={snapshot({
      "caption.font_size": { kind: "str", value: "small" },
      "caption.text_color": { kind: "str", value: "yellow" },
      "caption.frame_opacity": { kind: "int", value: 0 },
      "caption.translation_first": { kind: "bool", value: true },
    })} />));
    const sample = view.container.querySelector<HTMLElement>(".luna-caption-look")!;
    expect(sample.dataset.fontSize).toBe("small");
    expect(sample.dataset.textColor).toBe("yellow");
    expect(sample.dataset.order).toBe("translation-first");
    expect(sample.querySelector(".luna-subtitle__cue")?.firstElementChild?.className).toBe("luna-subtitle__translation");
    expect(sample.style.getPropertyValue("--caption-frame-alpha")).toBe("0");
    expect(view.container.querySelector('[aria-live="polite"]')).toBeNull();
    expect(view.container.querySelector(".luna-caption")).toBeNull();
    for (const command of ["listSettings", "setSetting", "recordingStart", "captionStart"] as const) {
      expect(commandMock(command)).not.toHaveBeenCalled();
    }
  });
  it("关闭翻译时仍显示原文，即使只看译文偏好已保存", () => {
    const view = render(withI18n(<CaptionSettingsPreview specs={snapshot({
      "caption.translation_enabled": { kind: "bool", value: false },
      "caption.show_source": { kind: "bool", value: false },
    })} />));
    expect(view.container.querySelectorAll(".luna-subtitle__source")).toHaveLength(1);
    expect(view.container.querySelector(".luna-subtitle__translation")).toBeNull();
  });
  it("分窗时原文和译文各处一窗，不重复双语内容", () => {
    const view = render(withI18n(<CaptionSettingsPreview specs={snapshot({
      "caption.separate_translation": { kind: "bool", value: true },
    })} />));
    const windows = view.container.querySelectorAll(".luna-caption-preview__window");
    expect(windows).toHaveLength(2);
    expect(windows[0].querySelector(".luna-subtitle__source")).not.toBeNull();
    expect(windows[0].querySelector(".luna-subtitle__translation")).toBeNull();
    expect(windows[1].querySelector(".luna-subtitle__source")).toBeNull();
    expect(windows[1].querySelector(".luna-subtitle__translation")).not.toBeNull();
  });
});
