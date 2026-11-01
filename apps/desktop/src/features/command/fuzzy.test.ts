import { describe, expect, it } from "vitest";
import { fuzzyMatch } from "./fuzzy";

describe("fuzzyMatch", () => {
  it("returns null when the query is not a subsequence", () => {
    expect(fuzzyMatch("abc", "acorn")).toBeNull();
  });

  it("matches case-insensitively and returns highlight ranges", () => {
    expect(fuzzyMatch("Qn", "Quick Notes")?.ranges).toEqual([
      [0, 1],
      [6, 7],
    ]);
  });

  it("scores contiguous matches higher than scattered matches", () => {
    const contiguous = fuzzyMatch("note", "Project note")?.score ?? 0;
    const scattered = fuzzyMatch("note", "n-o-t-e")?.score ?? 0;

    expect(contiguous).toBeGreaterThan(scattered);
  });

  it("weights path segment boundaries higher than mid-word matches", () => {
    const boundary = fuzzyMatch("mtg", "notes/meeting-guide.md")?.score ?? 0;
    const midWord = fuzzyMatch("mtg", "notes/something.md")?.score ?? 0;

    expect(boundary).toBeGreaterThan(midWord);
  });
});
