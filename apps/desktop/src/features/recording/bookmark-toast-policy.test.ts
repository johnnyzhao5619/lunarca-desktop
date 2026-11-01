import { describe, expect, it } from "vitest";
import { BOOKMARK_TOAST_DISMISS_MS, bookmarkAutoDismissMs } from "./bookmark-toast-policy";

describe("书签确认条自动收起（PRD-04-005）", () => {
  it("空备注且没在编辑：6 秒后收起", () => {
    expect(bookmarkAutoDismissMs({ note: "", editing: false })).toBe(BOOKMARK_TOAST_DISMISS_MS);
    expect(bookmarkAutoDismissMs({ note: "   ", editing: false })).toBe(BOOKMARK_TOAST_DISMISS_MS);
  });

  it("正在编辑：不收", () => {
    expect(bookmarkAutoDismissMs({ note: "", editing: true })).toBeNull();
  });

  it("备注有内容就不自动收——失焦想一想也不能把打了一半的字吃掉", () => {
    expect(bookmarkAutoDismissMs({ note: "重点", editing: false })).toBeNull();
  });
});
