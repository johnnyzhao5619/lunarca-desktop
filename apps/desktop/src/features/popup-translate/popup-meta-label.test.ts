import { describe, expect, it } from "vitest";
import type { PresetInfo, TranslateMeta } from "../../ipc/bindings";
import { i18n } from "../../i18n/i18n";
import { metaLabel } from "./popup-meta-label";

const PRESETS: PresetInfo[] = [
  { id: "general", name: "通用翻译", group: "style" },
  { id: "email", name: "邮件", group: "style" },
];

function meta(partial: Partial<TranslateMeta>): TranslateMeta {
  return { mode: "translate", target: "zh", uncertain: false, preset_id: "general", ...partial };
}

/** M1（判据 T9.3）：摘要行与下拉必须说同一种语言，语码要显示成语言名。 */
describe("弹窗翻译结果摘要行", () => {
  it("英文界面下预设名与目标语都是英文，不出现注册表里的中文名与裸语码", async () => {
    await i18n.changeLanguage("en-US");
    const t = i18n.getFixedT(null, "popup");
    const tCommon = i18n.getFixedT(null, "common");
    const label = metaLabel(meta({}), PRESETS, t, tCommon);
    expect(label).not.toContain("通用翻译");
    expect(label).toContain(tCommon("lang.zh"));
    expect(label).toContain(t("preset.general"));
    expect(label).not.toMatch(/\bzh\b/);
  });

  it("中文界面下同一份数据给出中文", async () => {
    await i18n.changeLanguage("zh-CN");
    const t = i18n.getFixedT(null, "popup");
    const tCommon = i18n.getFixedT(null, "common");
    const label = metaLabel(meta({ target: "en", preset_id: "email" }), PRESETS, t, tCommon);
    expect(label).toContain(tCommon("lang.en"));
    expect(label).toContain(t("preset.email"));
  });

  it("快照里找不到的预设 id 回落成 id 本身，而不是让整行消失", async () => {
    await i18n.changeLanguage("en-US");
    const t = i18n.getFixedT(null, "popup");
    const tCommon = i18n.getFixedT(null, "common");
    expect(metaLabel(meta({ preset_id: "vanished" }), PRESETS, t, tCommon)).toContain("vanished");
  });
});
