// 机器脚手架注释在实时预览里让位（ADR-024 索引卫生的呈现面对应物）。
//
// 真机症状：任何一篇录音转写笔记，正文第一行就摊着 `<!-- luna:transcript -->`。Rust 侧把这
// 对哨兵注释描述为「HTML 注释，渲染不可见」，而实时预览面同时也是编辑面——那个假设在这里
// 不成立。哨兵本身是**承重的**（`rewrite_recording_transcript` 靠它整块替换转写正文），
// 所以修的是渲染，不是内容。
//
// `@codemirror/state` 不碰 DOM，故判据与装饰范围都能在无 jsdom 的环境下真跑（仓库没有 jsdom）。

import { EditorState } from "@codemirror/state";
import { describe, expect, it } from "vitest";
import { buildScaffoldDecos, isLunaScaffoldComment } from "./editor-extensions";

function decoRanges(doc: string): Array<[number, number]> {
  const decos = buildScaffoldDecos(EditorState.create({ doc }));
  const out: Array<[number, number]> = [];
  const iter = decos.iter();
  while (iter.value) {
    out.push([iter.from, iter.to]);
    iter.next();
  }
  return out;
}

describe("isLunaScaffoldComment：只认整行、且属于 `luna:` 命名空间", () => {
  it("认得转写块的一对哨兵", () => {
    expect(isLunaScaffoldComment("<!-- luna:transcript -->")).toBe(true);
    expect(isLunaScaffoldComment("<!-- /luna:transcript -->")).toBe(true);
  });

  it("认得对话笔记的出处/补充标记——同一类机器脚手架，同一条规则覆盖", () => {
    // 这正是「按命名空间判定」的收益：没有把 chat.rs 那几个常量再抄一份过来。
    expect(isLunaScaffoldComment("<!-- luna:sources:v1:start -->")).toBe(true);
    expect(isLunaScaffoldComment("<!-- luna:model-supplement:v1:end -->")).toBe(true);
  });

  it("容忍缩进——哨兵可能被缩进，判据取 trim 后的整行", () => {
    expect(isLunaScaffoldComment("   <!-- luna:transcript -->  ")).toBe(true);
  });

  it("用户自己写的注释**不碰**", () => {
    // 藏掉用户写下的内容是这条规则唯一不可接受的失败方式。
    expect(isLunaScaffoldComment("<!-- TODO: 回头补一段 -->")).toBe(false);
    expect(isLunaScaffoldComment("<!-- lunar phase -->")).toBe(false);
    expect(isLunaScaffoldComment("<!-- lunatic -->")).toBe(false);
  });

  it("行内出现哨兵字样是**内容**，不是脚手架", () => {
    // 与 Rust `is_scaffold_line`「必须整行精确匹配」同一条纪律：有人在笔记里讨论这个标记
    // 本身时，那一行是他写的字。
    expect(isLunaScaffoldComment("见 <!-- luna:transcript --> 那一段")).toBe(false);
    expect(isLunaScaffoldComment("<!-- luna:a --> 和 <!-- luna:b -->")).toBe(false);
  });

  it("不完整或非注释的行一律不算", () => {
    expect(isLunaScaffoldComment("<!-- luna:transcript")).toBe(false);
    expect(isLunaScaffoldComment("luna:transcript -->")).toBe(false);
    expect(isLunaScaffoldComment("## User")).toBe(false);
    expect(isLunaScaffoldComment("")).toBe(false);
    expect(isLunaScaffoldComment("<!---->")).toBe(false);
  });
});

describe("buildScaffoldDecos：只盖住哨兵那几行", () => {
  it("转写正文与时间戳行一行都不动", () => {
    // 时间戳行是用户要读的内容（它们的可点跳转另属播放面）——这条扩展只管脚手架。
    const doc = [
      "<!-- luna:transcript -->",
      "[00:00:01.516] Howdy, Jack.",
      "[00:00:02.245] Hi, Jack.",
      "<!-- /luna:transcript -->",
    ].join("\n");

    const ranges = decoRanges(doc);
    expect(ranges).toHaveLength(2);

    const state = EditorState.create({ doc });
    // 第一条盖住第 1 行，第二条盖住第 4 行；中间两行原样。
    expect(ranges[0]).toEqual([state.doc.line(1).from, state.doc.line(1).to]);
    expect(ranges[1]).toEqual([state.doc.line(4).from, state.doc.line(4).to]);
  });

  it("没有脚手架的普通笔记不产生任何装饰", () => {
    expect(decoRanges("# 标题\n\n正文一段。\n")).toHaveLength(0);
  });

  it("用户写的注释行不产生装饰", () => {
    expect(decoRanges("<!-- TODO -->\n正文\n")).toHaveLength(0);
  });
});
