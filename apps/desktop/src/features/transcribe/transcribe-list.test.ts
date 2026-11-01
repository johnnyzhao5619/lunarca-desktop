import { describe, expect, it } from "vitest";
import {
  DEFAULT_RECORDING_SORT,
  DEFAULT_TRANSCRIBE_SORT,
  TRANSCRIBE_SORT_OPTIONS,
  formatListDuration,
  timeGroupingFor,
  rowTimeFor,
} from "./transcribe-list";

describe("转写列表排序选项", () => {
  /**
   * 这里**不再**测排序结果：排序已经整体归 Rust（TDD-07 §4.1a），行为用例在
   * `crates/luna-vault/src/index.rs`（含 `name` 的码点序黄金夹具）。前端若再留一份排序
   * 实现，它排的只是已加载的那几页——那正是本批修掉的错。
   */
  it("四个排序键各有一条文案，键名与 Rust 的 `ListSort` 逐字对齐", () => {
    expect(TRANSCRIBE_SORT_OPTIONS.map(([key]) => key)).toEqual([
      "modified",
      "name",
      "duration",
      "created",
    ]);
    expect(TRANSCRIBE_SORT_OPTIONS.map(([, labelKey]) => labelKey)).toEqual([
      "sort.modified",
      "sort.name",
      "sort.duration",
      "sort.created",
    ]);
  });

  it("转写默认按最近修改（持续产出的流水）；录音默认按录制时间（它的身份时间，PRD-07-007）", () => {
    expect(DEFAULT_TRANSCRIBE_SORT).toBe("modified");
    expect(DEFAULT_RECORDING_SORT).toBe("created");
    for (const sort of [DEFAULT_TRANSCRIBE_SORT, DEFAULT_RECORDING_SORT]) {
      expect(TRANSCRIBE_SORT_OPTIONS.some(([key]) => key === sort)).toBe(true);
    }
  });
});

describe("时长呈现", () => {
  it("超过一小时给 h:mm:ss，否则 m:ss", () => {
    expect(formatListDuration(7_200_000)).toBe("2:00:00");
    expect(formatListDuration(125_000)).toBe("2:05");
  });

  it("0 / null / 非法值不渲染徽标", () => {
    expect(formatListDuration(0)).toBeNull();
    expect(formatListDuration(null)).toBeNull();
    expect(formatListDuration(-1)).toBeNull();
    expect(formatListDuration(Number.NaN)).toBeNull();
  });
});

describe("时间分组的取值（PRD-07-007）", () => {
  const row = { created_ms: 1_000, modified_ms: 2_000 };
  it("按录制时间排就按录制时间分桶，没记的退到修改时间", () => {
    expect(timeGroupingFor("created")?.(row)).toBe(1_000);
    expect(timeGroupingFor("created")?.({ created_ms: null, modified_ms: 2_000 })).toBe(2_000);
    expect(timeGroupingFor("modified")?.(row)).toBe(2_000);
  });
  it("按名称 / 时长排不分组", () => {
    expect(timeGroupingFor("name")).toBeUndefined();
    expect(timeGroupingFor("duration")).toBeUndefined();
  });
});

describe("rowTimeFor", () => {
  const row = { created_ms: 1_000, modified_ms: 5_000 };
  it("行上的时间与分组同一个时间", () => {
    expect(rowTimeFor("modified")(row)).toBe(5_000);
    expect(rowTimeFor("created")(row)).toBe(1_000);
  });
  it("不分组的排序档退到身份时间", () => {
    expect(rowTimeFor("name")(row)).toBe(1_000);
    expect(rowTimeFor("name")({ created_ms: null, modified_ms: 5_000 })).toBe(5_000);
  });
});

