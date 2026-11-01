import { describe, expect, it } from "vitest";
import type { TrashEntryDto } from "../../ipc/bindings";
import { trashComparator } from "./trash-sort";

function entry(over: Partial<TrashEntryDto> & { id: string }): TrashEntryDto {
  return {
    original_path: `Notes/${over.id}.md`,
    deleted_ms: "1000",
    is_dir: false,
    byte_size: "100",
    expires_in_days: null,
    ...over,
  };
}

const paths = (items: TrashEntryDto[], mode: Parameters<typeof trashComparator>[0]) =>
  [...items].sort(trashComparator(mode)).map((item) => item.id);

describe("trashComparator", () => {
  it("默认口径是最近删除在最上面", () => {
    const items = [
      entry({ id: "早", deleted_ms: "100" }),
      entry({ id: "晚", deleted_ms: "900" }),
      entry({ id: "中", deleted_ms: "500" }),
    ];
    expect(paths(items, "deleted")).toEqual(["晚", "中", "早"]);
  });

  it("名称走自然序：第 2 章排在第 10 章前面（与文件树同一口径）", () => {
    const items = [
      entry({ id: "b", original_path: "Notes/第10章.md" }),
      entry({ id: "a", original_path: "Notes/第2章.md" }),
    ];
    expect(paths(items, "name")).toEqual(["a", "b"]);
  });

  it("名称只看文件名，不被原目录深浅左右", () => {
    const items = [
      entry({ id: "deep", original_path: "A/B/C/aaa.md" }),
      entry({ id: "flat", original_path: "zzz.md" }),
    ];
    expect(paths(items, "name")).toEqual(["deep", "flat"]);
  });

  it("占用倒序，且超过 Number 精度的体积也比得对", () => {
    const items = [
      entry({ id: "小", byte_size: "1" }),
      // 两个都超过 2^53，Number 会把它们看成相等。
      entry({ id: "大", byte_size: "9007199254740993" }),
      entry({ id: "更大", byte_size: "9007199254740995" }),
    ];
    expect(paths(items, "size")).toEqual(["更大", "大", "小"]);
  });

  // 同值项若不给稳定次级键，相对次序会随每次重渲染抖动。
  it("同值时按稳定次级键收尾", () => {
    const items = [
      entry({ id: "a", deleted_ms: "500", byte_size: "7" }),
      entry({ id: "b", deleted_ms: "500", byte_size: "7" }),
    ];
    expect(paths(items, "size")).toEqual(paths([...items].reverse(), "size"));
    expect(paths(items, "deleted")).toEqual(paths([...items].reverse(), "deleted"));
  });
});
