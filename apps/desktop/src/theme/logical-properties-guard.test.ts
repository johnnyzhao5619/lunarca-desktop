import { describe, expect, it } from "vitest";
import { physicalDirectionLocations } from "@lunarca/lint-config/css-guards";
import { reachableStyleSources, sourceLocation } from "./style-source";

describe("样式表守护 · 方向属性使用逻辑语义", () => {
  it("项目 CSS 不再写物理方向属性", () => {
    const offenders = reachableStyleSources.flatMap((source) =>
      physicalDirectionLocations(source.text, source.path).map(
        (entry) => `${sourceLocation(source, entry.line)} ${entry.property}`,
      ),
    );
    expect(offenders, `请改用 logical properties：\n${offenders.join("\n")}`).toEqual([]);
  });

  it("守护能抓住物理方向属性，也接受逻辑属性（反向验证）", () => {
    expect(physicalDirectionLocations(".fixture {\n  margin-left: 1rem;\n}")).toHaveLength(1);
    expect(physicalDirectionLocations(".fixture {\n  margin-inline-start: 1rem;\n  inset-block: 0;\n}")).toHaveLength(0);
  });
});
