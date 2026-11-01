import { describe, expect, it } from "vitest";
import zhWorkspace from "../../i18n/locales/zh-CN/workspace.json";
import enWorkspace from "../../i18n/locales/en-US/workspace.json";

/**
 * 蒸馏模板（PRD-06-051）的动态文案守护。
 *
 * 模板名、模板说明、建议判据三族文案都由 `` t(`distill.template.${id}`) `` 这类**变量拼出**
 * 的 key 查表——静态扫描 `t("…")` 的正则一个也看不到。漏写文案时界面上直接显示裸 key
 * （`distill.template.interview`），而 typecheck / lint / build 全部照绿。
 *
 * **模板清单本身由 Rust 下发**（`DistillOptionsDto.templates`，ADR-003），故本测试锁的是
 * 「Rust 可能发出的每个 id 都有文案」。Rust 侧枚举与这里的期望值由
 * `luna_core::distill_template` 的 `template_keys_round_trip_and_are_unique` 与
 * `distill_template_i18n_keys_exist`（Rust 直接读这两个 JSON）双向锁死——单靠前端无法
 * 感知 Rust 新增了模板，故那一半的守护必须住在 Rust 侧。
 */

type Dict = Record<string, unknown>;

function lookup(bundle: Dict, path: string): string | undefined {
  const value = path.split(".").reduce<unknown>((node, key) => {
    return typeof node === "object" && node !== null ? (node as Dict)[key] : undefined;
  }, bundle);
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

describe("蒸馏模板的动态文案（PRD-06-051）", () => {
  it("模板名与模板说明的 key 集完全对齐（中英各自成对）", () => {
    // 少一条说明 = 那个模板在浮层里只有名字没有解释，用户无从判断该不该选它。
    for (const bundle of [zhWorkspace, enWorkspace] as unknown as Dict[]) {
      const distill = (bundle as Dict).distill as Dict;
      const names = Object.keys(distill.template as Dict).sort();
      const descriptions = Object.keys(distill.templateDesc as Dict).sort();
      expect(names).toEqual(descriptions);
      expect(names.length).toBeGreaterThan(0);
    }
  });

  it("中英模板/判据的 key 集一致", () => {
    for (const group of ["template", "templateDesc", "reason"]) {
      const zh = Object.keys((zhWorkspace.distill as unknown as Dict)[group] as Dict).sort();
      const en = Object.keys((enWorkspace.distill as unknown as Dict)[group] as Dict).sort();
      expect(zh, `distill.${group} 中英键集不一致`).toEqual(en);
    }
  });

  it("选项浮层的固定文案在中英都在场", () => {
    for (const key of [
      "distill.optionsLabel",
      "distill.suggestedAs",
      "distill.templateLegend",
      "distill.fullText",
      "distill.fullTextDesc",
      "distill.fullTextCost",
      "distill.optionsCancel",
      "distill.optionsStart",
    ]) {
      for (const bundle of [zhWorkspace, enWorkspace] as unknown as Dict[]) {
        expect(lookup(bundle, key), `缺 ${key}`).toBeTruthy();
      }
    }
  });
});
