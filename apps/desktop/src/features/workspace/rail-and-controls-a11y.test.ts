/// <reference types="vite/client" />
// 键盘与读屏可达性：活动栏、速记角标、有序多选（ADR-087 / PRD-00 §7 / PRD-09-070）。
//
// 仓库没有 jsdom（见 `draft-distill-entry.test.ts` 先例），故按源码断言——这里要拦的
// 恰好都是源码层面的复发：tooltip 与可访问名分叉、角标被念第二遍、重排只剩拖拽。

import { describe, expect, it } from "vitest";

const SOURCES = import.meta.glob(
  "../../{features/workspace/WorkspacePrimaryRail.tsx,features/settings/SettingControls.tsx}",
  { eager: true, import: "default", query: "?raw" },
) as Record<string, string>;

const RAIL = Object.entries(SOURCES).find(([p]) => p.includes("PrimaryRail"))?.[1] as
  | string
  | undefined;
const CONTROLS = Object.entries(SOURCES).find(([p]) => p.includes("SettingControls"))?.[1] as
  | string
  | undefined;

function code(source: string): string {
  return source
    .replace(/\/\*[\s\S]*?\*\//g, "\n")
    .split("\n")
    .filter((line) => !/^\s*(\/\/|\*)/.test(line))
    .join("\n");
}

describe("活动栏：tooltip 与可访问名同源（PRD-00 §7 / PRD-07-002）", () => {
  it("扫描面非空", () => {
    expect(RAIL?.length ?? 0).toBeGreaterThan(500);
  });

  it("`title` 与 `aria-label` 是**同一个值**", () => {
    // 两者各算各的，鼠标看到的和读屏听到的就会是两句话——而它们描述的是同一颗按钮。
    // 带角标时那个值本身已经含了「· N 篇待处理」，故仍然只有一句。
    const body = code(RAIL ?? "");
    expect(body).toContain("aria-label={label}");
    expect(body).toContain("title={label}");
  });

  it("数字角标 `aria-hidden`——它已经在可访问名里说过一遍了", () => {
    // 不藏的话读屏会念两次数字（一次在按钮名里、一次在角标上），而它们是同一件事。
    const body = code(RAIL ?? "");
    const badge = body.slice(body.indexOf("luna-workspace__rail-badge"));
    expect(badge).toContain('aria-hidden="true"');
    // 悬浮标签同理：它是 `title`/`aria-label` 的视觉重复。
    expect(body).toContain('className="luna-workspace__rail-flyout" aria-hidden="true"');
  });
});

describe("有序多选：重排必须有键盘等价操作（PRD-00 §7 / PRD-09-070）", () => {
  it("扫描面非空", () => {
    expect(CONTROLS?.length ?? 0).toBeGreaterThan(1000);
  });

  it("上移/下移是**按钮**，不是只有拖拽", () => {
    // 纯拖拽的重排对键盘用户等于没有这个功能（PRD-00 §7）。
    const body = code(CONTROLS ?? "");
    expect(body).toContain('t("orderedMultiSelect.moveUp"');
    expect(body).toContain('t("orderedMultiSelect.moveDown"');
    expect(body).toContain("move(index, -1)");
    expect(body).toContain("move(index, 1)");
    // 全仓不得出现拖拽重排的痕迹——它只会成为键盘用户走不通的那条路。
    expect(body).not.toContain("draggable");
  });

  it("重排后由 `role=\"status\"` 报出新位置", () => {
    // 读屏用户按下"上移"之后必须知道它现在排第几，否则那颗按钮按下去没有任何反馈。
    const body = code(CONTROLS ?? "");
    expect(body).toContain('t("orderedMultiSelect.moved"');
    expect(body).toContain('role="status"');
    expect(body).toContain("position: target + 1");
  });

  it("两端的按钮禁用，不给按不动的假象", () => {
    const body = code(CONTROLS ?? "");
    // 有序多选：未选行没有位置可言，两颗箭头一并禁用；已选行只禁两端。
    expect(body).toContain("disabled={!picked || index === 0}");
    expect(body).toContain("disabled={!picked || index === selected.length - 1}");
    // 有序开关列表：全集恒在场，只禁两端。
    expect(body).toContain("disabled={index === 0}");
    expect(body).toContain("disabled={index === rows.length - 1}");
  });
});
