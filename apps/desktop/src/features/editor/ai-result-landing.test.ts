import { describe, expect, it } from "vitest";
import { orderedLandings } from "./AiResultPanel";

describe("orderedLandings", () => {
  it("puts the suggested landing first and keeps the rest of the scope's options", () => {
    expect(orderedLandings("selection", "insert_below")).toEqual(["insert_below", "replace"]);
    expect(orderedLandings("selection", "replace")).toEqual(["replace", "insert_below"]);
    expect(orderedLandings("doc", "insert_top")).toEqual(["insert_top", "insert_end"]);
  });

  it("falls back to the scope's default order when the suggestion is missing or foreign", () => {
    expect(orderedLandings("selection", null)).toEqual(["replace", "insert_below"]);
    expect(orderedLandings("cursor", "replace")).toEqual(["insert_cursor"]);
  });
});
