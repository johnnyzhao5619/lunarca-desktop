import { describe, expect, it } from "vitest";
import { assetMarkdown, isImageUrl, isSingleUrl } from "./paste-assets";

describe("isSingleUrl", () => {
  it("accepts a single http/https URL", () => {
    expect(isSingleUrl("https://example.com/a?b=1")).toBe(true);
    expect(isSingleUrl("  http://example.com  ")).toBe(true);
  });

  it("rejects non-URLs and multi-token text", () => {
    expect(isSingleUrl("see https://example.com now")).toBe(false);
    expect(isSingleUrl("ftp://example.com")).toBe(false);
    expect(isSingleUrl("just text")).toBe(false);
    expect(isSingleUrl("")).toBe(false);
  });
});

describe("isImageUrl", () => {
  it("accepts single http(s) URLs ending in an image extension", () => {
    expect(isImageUrl("https://example.com/a/photo.png")).toBe(true);
    expect(isImageUrl("http://x.com/pic.JPG")).toBe(true);
    expect(isImageUrl("https://cdn.com/i.webp?v=2")).toBe(true);
  });

  it("rejects non-image URLs and non-URLs", () => {
    expect(isImageUrl("https://example.com/page")).toBe(false);
    expect(isImageUrl("https://example.com/doc.pdf")).toBe(false);
    expect(isImageUrl("see https://x.com/a.png here")).toBe(false);
    expect(isImageUrl("assets/local.png")).toBe(false);
  });
});

describe("assetMarkdown", () => {
  it("builds an image embed for images", () => {
    expect(assetMarkdown("assets/Pasted-image-20260715.png", true)).toBe(
      "![](assets/Pasted-image-20260715.png)",
    );
  });

  it("builds a named link for non-image files", () => {
    expect(assetMarkdown("assets/report.pdf", false)).toBe("[report.pdf](assets/report.pdf)");
  });
});
