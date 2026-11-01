import { describe, expect, it } from "vitest";
import { noteStem, notePathLabel, parseNoteRoute } from "./note-path";

describe("noteStem / notePathLabel", () => {
  it("展示名去掉 .md 后缀而保留其它扩展名", () => {
    expect(noteStem("Notes/项目 X.md")).toBe("项目 X");
    expect(noteStem("Assets/图.png")).toBe("图.png");
    expect(noteStem("根笔记.md")).toBe("根笔记");
  });

  it("路径标签保留目录层级", () => {
    expect(notePathLabel("Notes/子/甲.md")).toBe("Notes/子/甲");
    expect(notePathLabel("Notes/子")).toBe("Notes/子");
  });
});

describe("parseNoteRoute", () => {
  it("解析行锚并保留路径", () => {
    expect(parseNoteRoute("notes://Notes/甲.md#L42")).toEqual({
      path: "Notes/甲.md",
      line: 42,
    });
  });

  it("无行锚时只给路径", () => {
    expect(parseNoteRoute("notes://Notes/甲.md")).toEqual({
      path: "Notes/甲.md",
      line: null,
    });
  });

  it("非法行锚当作路径的一部分——静默丢掉一段路径比不定位更糟", () => {
    expect(parseNoteRoute("notes://Notes/L型模板.md")).toEqual({
      path: "Notes/L型模板.md",
      line: null,
    });
    expect(parseNoteRoute("notes://Notes/甲.md#L0")).toEqual({
      path: "Notes/甲.md#L0",
      line: null,
    });
    expect(parseNoteRoute("notes://Notes/甲.md#Labc")).toEqual({
      path: "Notes/甲.md#Labc",
      line: null,
    });
  });

  it("文件名自身含 #L 时取最后一个作锚，不误伤前面的", () => {
    expect(parseNoteRoute("notes://Notes/关于#L的说明.md#L7")).toEqual({
      path: "Notes/关于#L的说明.md",
      line: 7,
    });
  });

  it("非 notes 协议与空路径不解析", () => {
    expect(parseNoteRoute("tags://x")).toBeNull();
    expect(parseNoteRoute("notes://")).toBeNull();
  });
});
