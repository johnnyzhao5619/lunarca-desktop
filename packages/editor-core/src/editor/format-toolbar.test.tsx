// @vitest-environment happy-dom
/**
 * 格式工具条（PRD-06-014、PRD-21-020）：桌面与网页同一条——条目同名同序；直接动作与「更多格式」按工具组
 * 固定分组（R5，2026-10-08），不量字宽：宽档面上留链接、图片 / 附件、表格；代码块、标注块、分隔线与 wiki
 * 链接在所有宽度都进菜单；宿主不能插附件（网页）时那一项哪儿都不出现。
 */
import { createElement } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { fakeEditorUi, t, withI18n } from "../test-fixture";
import type { FormatState } from "./format-commands";
import { FormatToolbar } from "./FormatToolbar";
import type { EditorFormatAction } from "./editor-actions";

const PLAIN: FormatState = {
  strong: false,
  em: false,
  del: false,
  code: false,
  heading: 0,
  bullet: false,
  ordered: false,
  task: false,
  quote: false,
};

function mount(options: { wide?: boolean; attachments?: boolean; formatState?: FormatState } = {}) {
  const onAction = vi.fn<(action: EditorFormatAction) => void>();
  render(
    withI18n(
      createElement(FormatToolbar, {
        formatState: options.formatState ?? PLAIN,
        onAction,
        ui: fakeEditorUi,
        wide: options.wide ?? true,
        attachments: options.attachments ?? true,
      }),
    ),
  );
  return onAction;
}

/** 工具条上直接可点的按钮（可访问名），按出现顺序。 */
function buttonNames(): string[] {
  const toolbar = screen.getByRole("toolbar", { name: t("editor.toolbar.label") });
  return [...toolbar.querySelectorAll(":scope > button, :scope > span > button")].map(
    (button) => button.getAttribute("aria-label") ?? button.textContent ?? "",
  );
}

afterEach(cleanup);

describe("格式工具条分组", () => {
  it("宽档：图片 / 附件与表格在面上，代码块收进更多格式", () => {
    mount({ wide: true });
    expect(screen.getByRole("button", { name: t("editor.menu.insertAttachment") })).toBeTruthy();
    expect(screen.getByRole("button", { name: t("editor.table.insert") })).toBeTruthy();
    expect(screen.queryByRole("button", { name: t("editor.menu.codeBlock") })).toBeNull();
    expect(screen.getByRole("button", { name: t("editor.toolbar.moreFormats") })).toBeTruthy();
  });

  it("窄档：只留表格，图片 / 附件也进菜单", () => {
    mount({ wide: false });
    expect(screen.getByRole("button", { name: t("editor.table.insert") })).toBeTruthy();
    expect(screen.queryByRole("button", { name: t("editor.menu.insertAttachment") })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: t("editor.toolbar.moreFormats") }));
    expect(screen.getByRole("menuitem", { name: t("editor.menu.insertAttachment") })).toBeTruthy();
  });

  it("条目同名同序：段落样式、四种行内格式、四种块、链接、表格、更多格式", () => {
    mount({ attachments: false });
    expect(buttonNames()).toEqual([
      t("editor.menu.paragraphText"),
      t("editor.menu.bold"),
      t("editor.menu.italic"),
      t("editor.menu.strike"),
      t("editor.menu.inlineCode"),
      t("editor.menu.quote"),
      t("editor.menu.bulletList"),
      t("editor.menu.orderedList"),
      t("editor.menu.taskList"),
      t("editor.menu.insertLink"),
      t("editor.table.insert"),
      t("editor.toolbar.moreFormats"),
    ]);
  });

  it("宿主不能插附件：面上与菜单里都没有「图片 / 附件」", () => {
    mount({ wide: true, attachments: false });
    expect(screen.queryByRole("button", { name: t("editor.menu.insertAttachment") })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: t("editor.toolbar.moreFormats") }));
    const items = screen.getAllByRole("menuitem").map((item) => item.textContent);
    expect(items).toEqual([t("editor.menu.insertWikiLink"), t("editor.menu.codeBlock"), t("editor.menu.callout"), t("editor.menu.hr")]);
  });
});

describe("格式工具条动作", () => {
  it("加粗按钮派发行内格式动作，激活态以 aria-pressed 呈现", () => {
    const onAction = mount({ formatState: { ...PLAIN, strong: true } });
    const bold = screen.getByRole("button", { name: t("editor.menu.bold") });
    expect(bold.getAttribute("aria-pressed")).toBe("true");
    expect(screen.getByRole("button", { name: t("editor.menu.italic") }).getAttribute("aria-pressed")).toBe("false");
    fireEvent.click(bold);
    expect(onAction).toHaveBeenCalledWith({ kind: "inline", mark: "strong" });
  });

  it("按下不夺走编辑器焦点（mousedown 默认行为被拦下）", () => {
    mount();
    const bold = screen.getByRole("button", { name: t("editor.menu.bold") });
    const event = new MouseEvent("mousedown", { bubbles: true, cancelable: true });
    bold.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
  });

  it("提示带快捷键（从命令注册表派生）", () => {
    mount();
    const title = screen.getByRole("button", { name: t("editor.menu.bold") }).getAttribute("title") ?? "";
    expect(title.startsWith(t("editor.menu.bold"))).toBe(true);
    expect(title).not.toBe(t("editor.menu.bold"));
  });

  it("段落样式菜单：当前级别打勾，选一项派发标题动作", () => {
    const onAction = mount({ formatState: { ...PLAIN, heading: 2 } });
    fireEvent.click(screen.getByRole("button", { name: t("editor.menu.h2") }));
    const h2 = screen.getByRole("menuitemcheckbox", { name: t("editor.menu.h2") });
    expect(h2.getAttribute("aria-checked")).toBe("true");
    fireEvent.click(screen.getByRole("menuitemcheckbox", { name: t("editor.menu.h3") }));
    expect(onAction).toHaveBeenCalledWith({ kind: "heading", level: 3 });
  });
});
