/**
 * 联网指示的两态守护（PRD-04-060 / ADR-111 ⑤⑴，2026-09-07）。
 *
 * 空闲态此前 aria 照样说「音频正在离开本机」——语音 HUD 空闲时也挂着这枚指示，那句话
 * 是假的。现在两态各有文案，且不只靠颜色区分（`data-active` 驱动实心 / 描边）。
 */
import { createElement } from "react";
import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { createInstance, type i18n as I18nInstance } from "i18next";
import { I18nextProvider } from "react-i18next";

import recording from "../../i18n/locales/zh-CN/recording.json";
import { HudUplinkDot } from "./HudUplinkDot";

const i18n: I18nInstance = createInstance();
void i18n.init({
  lng: "zh-CN",
  fallbackLng: "zh-CN",
  defaultNS: "recording",
  resources: { "zh-CN": { recording } },
  interpolation: { escapeValue: false },
});

function render(props: { name: string | null; active?: boolean }): string {
  return renderToStaticMarkup(
    createElement(I18nextProvider, { i18n }, createElement(HudUplinkDot, props)),
  );
}

describe("联网指示两态", () => {
  it("采集中：实心态，文案说「正在离开」并带执行者名", () => {
    const html = render({ name: "Lunarca 云", active: true });
    expect(html).toContain('data-active="true"');
    expect(html).toContain("正在离开本机");
    expect(html).toContain("Lunarca 云");
  });

  it("空闲：描边态，文案改为「说话时将交给…」，绝不说「正在离开」", () => {
    const html = render({ name: "Lunarca 云", active: false });
    expect(html).toContain('data-active="false"');
    expect(html).toContain("说话时音频将交给");
    expect(html).not.toContain("正在离开");
  });

  it("不传 active 视为采集中（既有三处挂载点行为不变）", () => {
    expect(render({ name: null })).toContain('data-active="true"');
  });

  it("字形是云上传，不再是三角形", () => {
    const html = render({ name: null });
    expect(html).toContain("<svg");
    expect(html).not.toContain("clip-path");
  });
});
