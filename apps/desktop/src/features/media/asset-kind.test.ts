import { describe, expect, it } from "vitest";
import { classifyAssetTarget, extensionOf } from "./asset-kind";

describe("extensionOf", () => {
  it("returns lowercase extension of the last path segment", () => {
    expect(extensionOf("Lunarca/assets/file-1.PNG")).toBe("png");
    expect(extensionOf("a/b/song.mp3")).toBe("mp3");
  });
  it("returns empty for no extension or dotfiles", () => {
    expect(extensionOf("Notes/folder")).toBe("");
    expect(extensionOf(".gitignore")).toBe("");
  });
});

describe("classifyAssetTarget", () => {
  it("classifies known attachment extensions", () => {
    expect(classifyAssetTarget("a/x.png")).toBe("image");
    expect(classifyAssetTarget("a/x.JPG")).toBe("image");
    expect(classifyAssetTarget("a/x.mp3")).toBe("audio");
    expect(classifyAssetTarget("a/x.mp4")).toBe("video");
    expect(classifyAssetTarget("a/x.pdf")).toBe("pdf");
    expect(classifyAssetTarget("a/x.docx")).toBe("binary");
  });

  it("returns null for notes, empty and folders (incl. dotted folder names)", () => {
    expect(classifyAssetTarget(null)).toBeNull();
    expect(classifyAssetTarget("a/note.md")).toBeNull();
    expect(classifyAssetTarget("Cardian/Cardian Academy v1.0.0")).toBeNull();
    expect(classifyAssetTarget("assets")).toBeNull();
    // 未知扩展交编辑器/文件夹卡处理，不误判为可外开二进制。
    expect(classifyAssetTarget("a/x.unknownext")).toBeNull();
  });
});
