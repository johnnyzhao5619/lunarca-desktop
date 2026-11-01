// 「这段源码是不是一张图」的判定已收到 `lib/markdown.ts::imageTargetOf`（语法树，两面
// 同源，TDD-15 §3.4a），用例在 `lib/markdown.test.ts`；本文件只剩「这个 src 能不能本地
// 加载」的离线纪律判定。
import { describe, expect, it } from "vitest";
import { isLocalAssetSrc } from "./image-embed";

describe("isLocalAssetSrc", () => {
  it("accepts vault-relative paths incl. parent traversal", () => {
    expect(isLocalAssetSrc("assets/x.png")).toBe(true);
    expect(isLocalAssetSrc("../Attachments/x.png")).toBe(true);
    expect(isLocalAssetSrc("x.png")).toBe(true);
  });

  it("rejects remote/data/anchor sources (offline discipline)", () => {
    expect(isLocalAssetSrc("https://example.com/x.png")).toBe(false);
    expect(isLocalAssetSrc("http://x/x.png")).toBe(false);
    expect(isLocalAssetSrc("data:image/png;base64,AAA")).toBe(false);
    expect(isLocalAssetSrc("#anchor")).toBe(false);
    expect(isLocalAssetSrc("")).toBe(false);
  });
});
