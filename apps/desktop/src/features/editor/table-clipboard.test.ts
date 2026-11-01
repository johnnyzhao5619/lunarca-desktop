import { describe, expect, it } from "vitest";
import { tablePlainCells, toCsv, toTsv } from "./table-clipboard";

describe("复制表格为 CSV / 带格式的纯文本那份", () => {
  const source = "| 名称 | 备注 |\n| --- | --- |\n| **乙**, 丙 | 说\"好\"<br>换行 |\n| `code` | a\\|b |";

  it("单元格取去掉 Markdown 标记的文字，<br> 是换行，转义的竖线还原", () => {
    expect(tablePlainCells(source)).toEqual([
      ["名称", "备注"],
      ["乙, 丙", "说\"好\"\n换行"],
      ["code", "a|b"],
    ]);
    expect(tablePlainCells("不是表格")).toBeNull();
  });

  it("CSV 按 RFC 4180 加引号；TSV 把格内换行压成空格", () => {
    const cells = tablePlainCells(source)!;
    expect(toCsv(cells)).toBe('名称,备注\r\n"乙, 丙","说""好""\n换行"\r\ncode,a|b');
    expect(toTsv(cells)).toBe("名称\t备注\n乙, 丙\t说\"好\" 换行\ncode\ta|b");
  });
});
