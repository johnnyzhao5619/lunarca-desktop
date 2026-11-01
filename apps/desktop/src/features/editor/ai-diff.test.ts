import { describe, expect, it } from "vitest";
import { applyChanges, diffChanges, diffSegments, DIFF_MAX_TOKENS, sentenceAround } from "./ai-diff";

describe("diffChanges", () => {
  it("returns nothing for identical text", () => {
    expect(diffChanges("abc", "abc")).toEqual([]);
  });

  it("finds a single CJK character substitution", () => {
    const changes = diffChanges("他跑的很快。", "他跑得很快。");
    expect(changes).toHaveLength(1);
    expect(changes[0]).toMatchObject({ from: 2, to: 3, original: "的", replacement: "得" });
  });

  it("merges adjacent word edits into one change and keeps whitespace attached", () => {
    const original = "She recieved the package and its contents was damaged.";
    const corrected = "She received the package and its contents were damaged.";
    const changes = diffChanges(original, corrected);
    expect(changes.map((c) => [c.original.trim(), c.replacement.trim()])).toEqual([
      ["recieved", "received"],
      ["was", "were"],
    ]);
    expect(applyChanges(original, changes, new Set(changes.map((c) => c.id)))).toBe(corrected);
  });

  it("handles pure insertions and deletions", () => {
    const changes = diffChanges("终于把问题解决，项目上线了。", "终于把问题解决了，项目上线了。");
    expect(changes).toHaveLength(1);
    expect(changes[0]).toMatchObject({ original: "", replacement: "了" });
    const removed = diffChanges("谢谢配合合。", "谢谢配合。");
    expect(removed).toHaveLength(1);
    expect(removed[0]).toMatchObject({ original: "合", replacement: "" });
  });

  it("applies only the accepted changes", () => {
    const original = "Its important to seperate them.";
    const corrected = "It's important to separate them.";
    const changes = diffChanges(original, corrected);
    expect(changes).toHaveLength(2);
    expect(applyChanges(original, changes, new Set([changes[1].id]))).toBe(
      "Its important to separate them.",
    );
    expect(applyChanges(original, changes, new Set())).toBe(original);
  });

  it("falls back to a single whole-text change when the input is huge", () => {
    const original = "字".repeat(DIFF_MAX_TOKENS + 1);
    const changes = diffChanges(original, `${original}。`);
    expect(changes).toHaveLength(1);
    expect(changes[0]).toMatchObject({ from: 0, to: original.length });
  });
});

describe("diffSegments", () => {
  it("interleaves untouched text with changes in document order", () => {
    const original = "a b c";
    const changes = diffChanges(original, "a x c");
    const segments = diffSegments(original, changes);
    expect(segments.map((s) => s.kind)).toEqual(["text", "change", "text"]);
  });
});

describe("sentenceAround", () => {
  it("expands to the enclosing sentence in Chinese and English", () => {
    const zh = "第一句。他跑的很快，一下子就到了终点。第三句。";
    const from = zh.indexOf("的");
    expect(sentenceAround(zh, from, from + 1)).toBe("他跑的很快，一下子就到了终点。");
    const en = "First one. She recieved it yesterday. Last one.";
    const at = en.indexOf("recieved");
    expect(sentenceAround(en, at, at + 8)).toBe("She recieved it yesterday.");
  });

  it("stops at line breaks and handles edges", () => {
    const text = "line one\nline two without period";
    const at = text.indexOf("two");
    expect(sentenceAround(text, at, at + 3)).toBe("line two without period");
  });
});
