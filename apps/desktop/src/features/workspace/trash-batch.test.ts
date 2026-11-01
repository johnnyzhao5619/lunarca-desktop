import { describe, expect, it } from "vitest";
import { summarizeRestoreBatch } from "./trash-batch";

describe("summarizeRestoreBatch", () => {
  it("全成功只说成功", () => {
    expect(summarizeRestoreBatch({ restored: 3, conflicted: 0, failed: 0 })).toEqual({
      key: "main.restoredBatch",
      params: { count: 3 },
    });
  });

  // 这一条就是真机 2026-09-09「只还原了其中一条、没有任何提示」的根因：三种去向各写
  // 一句状态栏，而状态栏只有一行——最后那句「已还原 1 项」把前面的都盖掉了，用户看到的
  // 是一句纯好消息，另外两条没回来这件事凭空消失。
  it("部分成功必须在同一句里说清有几条没回来", () => {
    expect(summarizeRestoreBatch({ restored: 1, conflicted: 2, failed: 0 })).toEqual({
      key: "main.restoreBatchPartial",
      params: { restored: 1, missed: 2 },
    });
    expect(summarizeRestoreBatch({ restored: 1, conflicted: 1, failed: 1 })).toEqual({
      key: "main.restoreBatchPartial",
      params: { restored: 1, missed: 2 },
    });
  });

  it("一条都没回来时直说没成，不用「已还原 0 项」绕着说", () => {
    expect(summarizeRestoreBatch({ restored: 0, conflicted: 2, failed: 0 })).toEqual({
      key: "main.restoreBatchAllConflicted",
      params: { count: 2 },
    });
    expect(summarizeRestoreBatch({ restored: 0, conflicted: 0, failed: 3 })).toEqual({
      key: "main.restoreBatchAllFailed",
      params: { count: 3, reason: "" },
    });
    // 撞名与真失败混在一起时不分开说两句——状态栏只有一行，合并成"有几条没回来"。
    expect(summarizeRestoreBatch({ restored: 0, conflicted: 1, failed: 2 })).toEqual({
      key: "main.restoreBatchAllFailed",
      params: { count: 3, reason: "" },
    });
  });

  // 一条都没回来时用户手里没有任何别的线索，一句「3 项未能还原」等于让他去猜是磁盘
  // 只读还是权限不足。部分成功时不带原因——他可以对剩下那几条单独再试，那时有精确报错。
  it("一条都没回来时带上原因，部分成功时不带", () => {
    expect(
      summarizeRestoreBatch({ restored: 0, conflicted: 0, failed: 2 }, "磁盘不可写"),
    ).toEqual({ key: "main.restoreBatchAllFailed", params: { count: 2, reason: "磁盘不可写" } });
    expect(summarizeRestoreBatch({ restored: 1, conflicted: 0, failed: 2 }, "磁盘不可写")).toEqual({
      key: "main.restoreBatchPartial",
      params: { restored: 1, missed: 2 },
    });
  });

  it("什么都没发生就不写状态栏", () => {
    expect(summarizeRestoreBatch({ restored: 0, conflicted: 0, failed: 0 })).toBeNull();
  });
});
