// @vitest-environment happy-dom
/**
 * 格式工具栏的直接动作与「更多格式」分组（R5，2026-10-08）：按工具组固定分组，不量字宽。
 * 宽档面上留链接、图片 / 附件、表格；代码块、标注块、分隔线与 wiki 链接在所有宽度都进菜单。
 */
import { createElement } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import { ContextMenuHost } from "../../components/ContextMenuHost";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import type { EditorTier } from "./editor-tier";
import type { FormatState } from "./format-commands";
import { FormatToolbar } from "./FormatToolbar";

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

function mount(tier: EditorTier) {
  render(
    withI18n(
      createElement(
        ContextMenuHost,
        null,
        createElement(FormatToolbar, { formatState: PLAIN, onAction: vi.fn(), tier }),
      ),
    ),
  );
}

afterEach(cleanup);

describe("格式工具栏分组", () => {
  it("宽档：图片 / 附件与表格在面上，代码块收进更多格式", () => {
    mount("wide");
    expect(screen.getByRole("button", { name: t("editor:menu.insertAttachment") })).toBeTruthy();
    expect(screen.getByRole("button", { name: t("editor:table.insert") })).toBeTruthy();
    expect(screen.queryByRole("button", { name: t("editor:menu.codeBlock") })).toBeNull();
    expect(screen.getByRole("button", { name: t("editor:more.formats") })).toBeTruthy();
  });

  it("窄档：只留表格，图片 / 附件也进菜单", () => {
    mount("narrow");
    expect(screen.getByRole("button", { name: t("editor:table.insert") })).toBeTruthy();
    expect(screen.queryByRole("button", { name: t("editor:menu.insertAttachment") })).toBeNull();
  });
});
