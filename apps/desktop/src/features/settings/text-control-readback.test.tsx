// @vitest-environment happy-dom
/** 文本设置必须回显 Rust 归一化后的权威值，而不是继续展示用户提交的原始草稿。 */

import { createElement } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { cleanup, fireEvent, render, waitFor } from "@testing-library/react";

import { withI18n } from "../../i18n/i18n-test-fixture";
import type { SettingValueDto } from "../../ipc/bindings";
import { SettingControl } from "./SettingControls";
import { createSettingSpecFixture } from "./settings-test-fixture";

afterEach(cleanup);

describe("文本设置权威回显", () => {
  it("规范化结果等于编辑前值时仍替换省略协议的草稿", async () => {
    const canonical = "https://10.0.0.62:8443";
    const spec = {
      ...createSettingSpecFixture(),
      key: "transcribe.fetch_proxy",
      current_value: { kind: "str" as const, value: canonical },
      modified: false, available: true, default_value: { kind: "str" as const, value: "" },
    resettable: true,
      control: { kind: "text" as const, preview: null, multiline: false },
    };
    const view = render(
      withI18n(
        createElement(SettingControl, {
          spec,
          setValue: async () => ({ kind: "str" as const, value: canonical }),
          onVaultChanged: () => {},
        }),
      ),
    );
    const input = view.container.querySelector('input[type="text"]') as HTMLInputElement;

    fireEvent.change(input, { target: { value: "10.0.0.62:8443" } });
    fireEvent.blur(input);

    await waitFor(() => expect(input.value).toBe(canonical));
  });

  it("一段话（multiline）是多行框：回车是换行，离开时保存，回显 Rust 归一化后的值", async () => {
    const saved: string[] = [];
    const spec = {
      ...createSettingSpecFixture(),
      key: "creator.channel_voice",
      current_value: { kind: "str" as const, value: "" },
      modified: false, available: true, default_value: { kind: "str" as const, value: "" },
      resettable: true,
      control: { kind: "text" as const, preview: null, multiline: true },
    };
    const view = render(
      withI18n(
        createElement(SettingControl, {
          spec,
          setValue: async (_key: string, value: SettingValueDto) => {
            const text = value.kind === "str" ? value.value : "";
            saved.push(text);
            return { kind: "str" as const, value: text.trim() };
          },
          onVaultChanged: () => {},
        }),
      ),
    );
    expect(view.container.querySelector('input[type="text"]')).toBeNull();
    const area = view.container.querySelector("textarea") as HTMLTextAreaElement;
    fireEvent.change(area, { target: { value: "面向程序员\n说话直接 " } });
    fireEvent.keyDown(area, { key: "Enter" });
    expect(saved).toEqual([]);
    fireEvent.blur(area);
    await waitFor(() => expect(area.value).toBe("面向程序员\n说话直接"));
    expect(saved).toEqual(["面向程序员\n说话直接 "]);
  });
});
