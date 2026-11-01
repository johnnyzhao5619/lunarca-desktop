import { describe, expect, it } from "vitest";
import type { SyncOverviewDto, VaultSyncDto } from "../../ipc/bindings";
import { projectStatusBarSync } from "./statusbar-sync-status";

function vault(overrides: Partial<VaultSyncDto> = {}): VaultSyncDto {
  return {
    path: "/v/a",
    name: "工作",
    is_open: true,
    phase: "idle",
    paused: false,
    remote_vault_id: "r1",
    last_synced_ms: 1_700_000_000_000,
    last_error: null,
    conflict_copies: 0,
    held_deletions: 0,
    too_large: [],
    display_path: "~/v/a",
    local_bytes: null,
    cloud_bytes: null,
    bound_account: null,
    ...overrides,
  };
}

function overview(vaults: VaultSyncDto[], overrides: Partial<SyncOverviewDto> = {}): SyncOverviewDto {
  return {
    gate: "ready",
    paused: null,
    storage: null,
    read_only: null,
    large_paused: null,
    account_frozen: false,
    vaults,
    ...overrides,
  };
}

describe("projectStatusBarSync", () => {
  it("一个库都没选中、或整体不可用时不占底栏", () => {
    expect(projectStatusBarSync(null).kind).toBe("hidden");
    expect(projectStatusBarSync(overview([vault({ phase: "off" })])).kind).toBe("hidden");
    expect(projectStatusBarSync(overview([vault()], { gate: "signed_out" })).kind).toBe("hidden");
    expect(projectStatusBarSync(overview([vault()], { gate: "strict_local" })).kind).toBe("hidden");
  });

  it("只停了大文件（ADR-156 ③）：说出原因，排在整体暂停之后、同步中之前", () => {
    expect(projectStatusBarSync(overview([vault()], { large_paused: "metered" })).kind).toBe(
      "large_paused_metered",
    );
    expect(
      projectStatusBarSync(overview([vault({ phase: "syncing" })], { large_paused: "low_battery" })).kind,
    ).toBe("large_paused_battery");
    expect(
      projectStatusBarSync(
        overview([vault()], { large_paused: "metered", paused: { until_ms: null } }),
      ).kind,
    ).toBe("paused");
  });

  it("连在另一个账户下的库不算选中：不占底栏、不计数（ADR-151 修订 2026-10-15）", () => {
    const elsewhere = vault({ path: "/v/home", phase: "other_account", bound_account: "jo***@gmail.com" });
    expect(projectStatusBarSync(overview([elsewhere])).kind).toBe("hidden");
    expect(projectStatusBarSync(overview([vault(), elsewhere]))).toEqual({ kind: "synced", count: 1 });
  });

  it("全部已同步时报出选中的库数（未选中的不算）", () => {
    expect(
      projectStatusBarSync(overview([vault(), vault({ path: "/v/b" }), vault({ phase: "off" })])),
    ).toEqual({ kind: "synced", count: 2 });
  });

  it("要用户动手的那件事压过安静的状态，跨库聚合", () => {
    const busy = vault({ path: "/v/b", phase: "syncing" });
    expect(
      projectStatusBarSync(overview([busy, vault({ held_deletions: 61 }), vault({ held_deletions: 4 })])),
    ).toEqual({ kind: "awaiting_confirmation", count: 65 });
    expect(projectStatusBarSync(overview([busy, vault({ phase: "unavailable" })])).kind).toBe(
      "unavailable",
    );
    expect(projectStatusBarSync(overview([busy, vault({ phase: "locked" })])).kind).toBe("locked");
  });

  it("出错按码分成只读 / 离线 / 其他", () => {
    const err = (code: string) =>
      projectStatusBarSync(overview([vault({ phase: "error", last_error: code })])).kind;
    expect(err("sync_read_only")).toBe("read_only");
    expect(err("sync_quota_exceeded")).toBe("read_only");
    // 失去同步权益（退订 / 降级，PRD-13-015）：同样是「能看不能写」。
    expect(err("sync_not_entitled")).toBe("read_only");
    expect(err("network")).toBe("offline");
    expect(err("sync_remote_corrupt")).toBe("error");
  });

  it("账户被冻结（纵切 3 C3）：说出上传为什么停了，排在只读之后、离线之前", () => {
    expect(projectStatusBarSync(overview([vault()], { account_frozen: true })).kind).toBe(
      "account_frozen",
    );
    expect(
      projectStatusBarSync(
        overview([vault({ phase: "error", last_error: "sync_read_only" })], { account_frozen: true }),
      ).kind,
    ).toBe("read_only");
    expect(
      projectStatusBarSync(
        overview([vault({ phase: "error", last_error: "network" })], { account_frozen: true }),
      ).kind,
    ).toBe("account_frozen");
  });

  it("全部暂停，或选中的每个库都单独暂停了，才算暂停", () => {
    expect(projectStatusBarSync(overview([vault()], { paused: { until_ms: 9 } })).kind).toBe("paused");
    expect(
      projectStatusBarSync(overview([vault({ phase: "paused", paused: true })])).kind,
    ).toBe("paused");
    expect(
      projectStatusBarSync(
        overview([vault({ phase: "paused", paused: true }), vault({ path: "/v/b", phase: "syncing" })]),
      ).kind,
    ).toBe("syncing");
  });

  it("冲突副本跨库合计", () => {
    expect(
      projectStatusBarSync(overview([vault({ conflict_copies: 2 }), vault({ conflict_copies: 1 })])),
    ).toEqual({ kind: "conflicts", count: 3 });
  });
});
