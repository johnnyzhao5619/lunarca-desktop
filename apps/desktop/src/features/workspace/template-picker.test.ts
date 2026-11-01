/// <reference types="vite/client" />
// 内置模板的名字与说明（PRD-06-015 / TDD-06 §10）。
//
// **这一族用例存在的理由**：模板卡的文案是 `t(\`template.${template.id}\`)` 这种
// **动态取键**——`id` 由 Rust `list_builtin()` 下发，静态 key 守护看不见它，locale parity
// 也只比"两个 locale 有没有同一批键"（少的那个两边一起少，照样对齐）。于是 `NoteType`
// 的 wire 值一旦改名，模板选择器就渲染出裸 key，而全套自动化一片绿。
//
// 真机验收（2026-08-26）逐字抓到的就是这个：`NoteType::Batch` 早已改名为 `Transcript`，
// i18n 里却还留着 `template.batch`/`template.batch_desc`，于是第四张卡上写着
// 「template.transcript / template.transcript_desc」两行裸 key，而且那份旧文案的内容
// 也早就不对了（写的是「批量转写设置」——一个设置页的名字，不是笔记模板）。

import { describe, expect, it } from "vitest";
import type { NoteTypeDto } from "../../ipc/bindings";
import zhWorkspace from "../../i18n/locales/zh-CN/workspace.json";
import enWorkspace from "../../i18n/locales/en-US/workspace.json";

/**
 * 每个 `NoteType` 一份内置模板（TDD-06 §10），此表**穷尽** `NoteTypeDto`。
 *
 * 写成 `Record<NoteTypeDto, true>` 而不是数组：Rust 增删或改名一个笔记类型，绑定重生成后
 * 这里**编译期**就红，而不是等到有人在真机上点开模板选择器才发现少了一张卡或多了一行裸 key。
 */
const BUILTIN_TEMPLATE_IDS: Record<NoteTypeDto, true> = {
  note: true,
  "quick-note": true,
  recording: true,
  transcript: true,
  chat: true,
  document: true,
  "video-project": true,
  script: true,
  swipe: true,
};

const BUNDLES: Record<string, { template?: Record<string, unknown> }> = {
  "zh-CN": zhWorkspace,
  "en-US": enWorkspace,
};

describe("内置模板文案（动态取键，静态守护看不见）", () => {
  it("每个笔记类型的名字与说明在中英双语都真的存在，且不是裸 key", () => {
    const missing: string[] = [];
    for (const id of Object.keys(BUILTIN_TEMPLATE_IDS)) {
      for (const [locale, bundle] of Object.entries(BUNDLES)) {
        for (const key of [`template.${id}`, `template.${id}_desc`]) {
          const value = bundle.template?.[key.slice("template.".length)];
          if (typeof value !== "string" || value.trim().length === 0 || value === key) {
            missing.push(`${locale}: ${key}`);
          }
        }
      }
    }
    expect(missing, `这些模板文案缺失或是裸 key：\n${missing.join("\n")}`).toEqual([]);
  });

  it("`template.*` 里没有对不上任何笔记类型的孤儿键", () => {
    // 反向：改名时**只加不删**会把旧键留成死文案，下一个人读 locale 会以为它还在用
    //（`template.batch` 正是这么活了下来）。这里只管 `<id>` / `<id>_desc` 这一族，
    // 面板自己的标题、提示、按钮文案不在其列。
    const structural = new Set([
      "eyebrow",
      "title",
      "cancel",
      "builtinTitle",
      "builtinHint",
      "packTitle",
      "packHint",
      "fromPack",
      "shadowsOthers",
      "userTitle",
      "userHint",
      "userEmpty",
      "manage",
      "manageError",
      "error",
      "variables",
    ]);
    const ids = new Set(Object.keys(BUILTIN_TEMPLATE_IDS));
    for (const [locale, bundle] of Object.entries(BUNDLES)) {
      const orphans = Object.keys(bundle.template ?? {}).filter((key) => {
        if (structural.has(key)) {
          return false;
        }
        const base = key.endsWith("_desc") ? key.slice(0, -"_desc".length) : key;
        return !ids.has(base);
      });
      expect(orphans, `${locale} 的 template.* 有孤儿键：${orphans.join(", ")}`).toEqual([]);
    }
  });
});
