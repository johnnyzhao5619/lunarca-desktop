/**
 * **Rust 下发的稳定串 ↔ i18n key 对账。**
 *
 * **拦的是一类只在真机上才现形的缺陷**：模型面板到处用 `` t(`models.<组>.${值}`) ``
 * 渲染 Rust 下发的稳定串（能力、槽位、就绪度、来源）。模板拼出来的 key **编译期查不到**，
 * 资源包里少一条时 i18next 回落成裸 key——用户读到的是「对话、models.capability.translate、
 * models.capability.summarize … 由服务端执行」，而 typecheck / lint / 既有单测全绿。
 * 2026-08-08 真机就是这么撞见的，本文件是那次之后立的账。
 *
 * 清单在这里**照抄一份是刻意的**：这些字段在 DTO 里是 `String`，前端拿不到穷尽类型，
 * 没有编译期能替代它的东西。抄漏一条的代价是界面上一行裸 key；抄多一条则立刻红
 * （资源包里查无此项），所以这份账不会悄悄腐烂。
 */

import { describe, expect, it } from "vitest";

import en from "../../i18n/locales/en-US/settings.json";
import zh from "../../i18n/locales/zh-CN/settings.json";

/** 各组的取值集合，对应 Rust 侧的同名产出处。 */
const GROUPS: Record<string, readonly string[]> = {
  // `models::taken_over_capabilities`（ipc/models.rs）+ 模型目录里出现的能力。
  "models.capability": ["chat", "translate", "summarize", "refine", "transcribe", "embed", "diarize"],
  // `luna_config::ModelSlot::as_str`。
  "models.slot": ["realtime_asr", "batch_asr", "default_llm", "realtime_llm", "default_embedding"],
  "models.slot_desc": [
    "realtime_asr",
    "batch_asr",
    "default_llm",
    "realtime_llm",
    "default_embedding",
  ],
  // `assignment_readiness`（ipc/mod.rs；DTO 注释同处列举）。
  "models.readiness": ["ready", "missing", "incompatible", "unresolved"],
  // `assignment_source`。
  "models.source_kind": ["explicit", "inherited", "device_policy"],
};

/** 按点号路径取值；缺任何一段即视为缺 key。 */
function lookup(bundle: unknown, path: string): unknown {
  return path.split(".").reduce<unknown>((node, part) => {
    if (node === null || typeof node !== "object") return undefined;
    return (node as Record<string, unknown>)[part];
  }, bundle);
}

describe("Rust 下发串 ↔ i18n key", () => {
  const cases = Object.entries(GROUPS).flatMap(([group, values]) =>
    values.map((value) => [`${group}.${value}`] as const),
  );

  it.each(cases)("%s 在中英资源包里都有文案", (key) => {
    expect(lookup(zh, key), `zh-CN 缺 ${key}`).toBeTypeOf("string");
    expect(lookup(en, key), `en-US 缺 ${key}`).toBeTypeOf("string");
  });
});
