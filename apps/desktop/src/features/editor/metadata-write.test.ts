import { describe, expect, it } from "vitest";
import { isMetadataOnlyWrite } from "./metadata-write";

const SAVED = "---\nluna:\n  schema: 1\n  type: script\n---\n## 开头\n你好\n";

describe("只改了元数据的外部写入", () => {
  it("后台补写 AI 标签：正文没动 → 是", () => {
    const tagged = "---\nluna:\n  schema: 1\n  type: script\n  ai_tags:\n  - 开场\n---\n## 开头\n你好\n";
    expect(isMetadataOnlyWrite(SAVED, tagged)).toBe(true);
  });

  it("没有 frontmatter 的笔记第一次被写进 luna 块：正文没动 → 是", () => {
    expect(isMetadataOnlyWrite("## 开头\n你好\n", "---\nluna:\n  ai_tags:\n  - x\n---\n## 开头\n你好\n")).toBe(true);
  });

  it("正文变了（外部编辑器改了字）→ 不是，照旧弹横幅", () => {
    expect(isMetadataOnlyWrite(SAVED, SAVED.replace("你好", "大家好"))).toBe(false);
  });

  it("两边一模一样 → 不是（那是自写回声，另有判据）", () => {
    expect(isMetadataOnlyWrite(SAVED, SAVED)).toBe(false);
  });
});
