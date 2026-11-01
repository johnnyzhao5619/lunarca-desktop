import { describe, expect, it } from "vitest";
import { reachableStyleSources, sourceLocation } from "./style-source";

const physicalProperty = /(?:^|[;{])\s*(?:left|right|top|bottom|margin-left|margin-right|padding-left|padding-right|border-left|border-right|border-top|border-bottom|inset-left|inset-right)\s*:/gm;

function directionLocations(css: string, sourcePath = "fixture.css") {
  const withoutComments = css.replace(/\/\*[\s\S]*?\*\//g, (comment) => "\n".repeat((comment.match(/\n/g) ?? []).length));
  return [...withoutComments.matchAll(physicalProperty)].map((match) => ({
    property: match[0].replace(/^[;{]\s*/, "").trim().slice(0, -1),
    line: (withoutComments.slice(0, match.index).match(/\n/g) ?? []).length + 1,
    sourcePath,
  }));
}

describe("样式表守护 · 方向属性使用逻辑语义", () => {
  it("项目 CSS 不再写物理方向属性", () => {
    const offenders = reachableStyleSources.flatMap((source) =>
      directionLocations(source.text, source.path).map(
        (entry) => `${sourceLocation(source, entry.line)} ${entry.property}`,
      ),
    );
    expect(offenders, `请改用 logical properties：\n${offenders.join("\n")}`).toEqual([]);
  });

  it("守护能抓住物理方向属性，也接受逻辑属性（反向验证）", () => {
    expect(directionLocations(".fixture {\n  margin-left: 1rem;\n}")).toHaveLength(1);
    expect(directionLocations(".fixture {\n  margin-inline-start: 1rem;\n  inset-block: 0;\n}")).toHaveLength(0);
  });
});
