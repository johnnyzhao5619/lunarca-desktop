import { describe, expect, it } from "vitest";
import { DRAG_THRESHOLD_PX, shouldStartDrag } from "./drag-threshold";

describe("指针拖拽起拖阈值", () => {
  it("位移小于阈值视作点击，绝不把单击变成误拖", () => {
    // 文件树与标签条的行既要能点开、又要能拖走：没有阈值的话，
    // 每一次带一两像素抖动的点击都会变成一次移动操作。
    expect(shouldStartDrag(0, 0)).toBe(false);
    expect(shouldStartDrag(2, 2)).toBe(false);
    expect(shouldStartDrag(-3, 0)).toBe(false);
  });

  it("达到阈值即起拖（含斜向，按欧氏距离而非单轴）", () => {
    expect(shouldStartDrag(DRAG_THRESHOLD_PX, 0)).toBe(true);
    expect(shouldStartDrag(0, -DRAG_THRESHOLD_PX)).toBe(true);
    expect(shouldStartDrag(3, 3)).toBe(true); // hypot ≈ 4.24
  });

  it("阈值可覆写（不同交互可有不同手感）", () => {
    expect(shouldStartDrag(5, 0, 10)).toBe(false);
    expect(shouldStartDrag(12, 0, 10)).toBe(true);
  });
});
