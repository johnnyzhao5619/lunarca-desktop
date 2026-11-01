// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { PastePreviewDialog } from "./PastePreviewDialog";
import type { HtmlPasteDto } from "../../ipc/bindings";
vi.mock("react-i18next", () => ({ useTranslation: () => ({ t: (key: string) => key }) }));
afterEach(cleanup);
const preview: HtmlPasteDto = { markdown: "blank-header-table", warnings: ["missing_header"], block: true,
  header_markdown: "first-row-header-table", table_cells: [["标题", "数量"]] };
describe("paste preview choices", () => {
  it("默认保留首行数据，选择表头后预览与最终输出同步；新预览重新默认保留数据", () => {
    const onResolve = vi.fn();
    const { rerender } = render(<PastePreviewDialog preview={preview} plain={"标题\t数量"} onResolve={onResolve} />);
    const checkbox = screen.getByRole("checkbox") as HTMLInputElement;
    expect(checkbox.checked).toBe(false);
    fireEvent.click(checkbox);
    expect(screen.getByText("first-row-header-table")).toBeTruthy();
    expect(screen.queryByText("paste.warning.missing_header")).toBeNull();
    fireEvent.click(screen.getByText("paste.insertConverted"));
    expect(onResolve).toHaveBeenCalledWith({ kind: "converted", markdown: "first-row-header-table" });
    rerender(<PastePreviewDialog preview={{ ...preview }} plain={"标题\t数量"} onResolve={onResolve} />);
    expect((screen.getByRole("checkbox") as HTMLInputElement).checked).toBe(false);
    expect(screen.getByText("blank-header-table")).toBeTruthy();
  });
  it("填充模式显示影响范围，提供纯文本与取消，不显示表头选择", () => {
    const onResolve = vi.fn(); render(<PastePreviewDialog preview={preview} plain={"标题\t数量"} onResolve={onResolve}
      tableTarget={{ row: 2, column: 2, rows: 2, columns: 2, replacedCells: 1, addedRows: 1, addedColumns: 1 }} />);
    expect(screen.queryByRole("checkbox")).toBeNull(); expect(screen.getByText("paste.tableFillChanges")).toBeTruthy();
    fireEvent.click(screen.getByText("paste.insertPlain")); expect(onResolve).toHaveBeenLastCalledWith({ kind: "plain", text: "标题\t数量" });
    fireEvent.click(screen.getByText("paste.cancel")); expect(onResolve).toHaveBeenLastCalledWith(null);
  });
});
