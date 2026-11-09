import { describe, expect, it } from "vitest";
import { scanLineForTags } from "./inline-tags";

describe("scanLineForTags（与 Rust extract_inline_tags 同构）", () => {
  it("识别行首/空白后的 #tag，含嵌套与 CJK", () => {
    const matches = scanLineForTags("#工作 中间 #project/luna 与 #re-check_2");
    expect(matches.map((m) => m.tag)).toEqual(["工作", "project/luna", "re-check_2"]);
  });

  it("返回的偏移覆盖 `#` 到标签体末尾", () => {
    const [m] = scanLineForTags("见 #tag 后文");
    expect(m.from).toBe(2);
    expect(m.to).toBe(6);
  });

  it("拒绝标题、纯数字与词中 #", () => {
    expect(scanLineForTags("# 标题")).toEqual([]);
    expect(scanLineForTags("序号 #123")).toEqual([]);
    expect(scanLineForTags("词中a#b")).toEqual([]);
  });

  it("跳过行内 code span，尾部斜杠不计入标签体", () => {
    expect(scanLineForTags("`#code` 之外 #real/").map((m) => m.tag)).toEqual(["real"]);
    const [m] = scanLineForTags("#a/");
    expect(m.to).toBe(2);
  });

  it("拒绝以斜杠开头的标签体", () => {
    expect(scanLineForTags("#/oops")).toEqual([]);
  });
});
