import { describe, expect, it } from "vitest";
import { shouldReportSuggestion } from "./use-suggestion-visibility";

describe("suggestion visibility boundary", () => {
  it("prefetch or a card hidden behind running maintenance is not an impression", () => {
    expect(shouldReportSuggestion(false, true, false)).toBe(false);
  });

  it("a background document is not an impression", () => {
    expect(shouldReportSuggestion(true, false, false)).toBe(false);
  });

  it("an actually visible card reports once", () => {
    expect(shouldReportSuggestion(true, true, false)).toBe(true);
    expect(shouldReportSuggestion(true, true, true)).toBe(false);
  });

  it("does not race a duplicate while persistence is still pending", () => {
    expect(shouldReportSuggestion(true, true, false, true)).toBe(false);
    expect(shouldReportSuggestion(true, true, false, false)).toBe(true);
  });
});
