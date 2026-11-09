// @vitest-environment happy-dom
/**
 * 账户页「云同步」一节与开启向导（ADR-148、ADR-150、ADR-151）。
 *
 * 钉只有真渲染才看得见的几件事：
 * 1. 逐库选择：每个已知库一行一个开关，开 = 向导（新建 / 加入），关 = 确认后停止同步；
 * 2. 暂停两级：全部暂停可选时长、暂停中给「恢复」；单库暂停只动那一行；
 * 3. 状态跟着 `sync://status` 走（与状态栏同一份总览）；
 * 4. 同步跟着账户走：向导里没有任何密码框；明确选择新建或加入、按库名认，已在这台设备上
 *    同步的云端库不再列出；
 * 5. 失败按稳定码出文案，「上次同步」按毫秒格式化；
 * 6. 连在另一个账户下的库（ADR-151 修订 2026-10-15）：开关为关、写明连在谁那儿、不给暂停与恢复；
 *    「改为同步到当前账户」先确认后果，确认了才开向导，开启时带上改连。
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, waitFor } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import {
  commandMock,
  emitEvent,
  resetBindingsDouble,
  setCommandReply,
} from "../../ipc/bindings-test-double";
import type { RemoteVaultDto, SyncOverviewDto, VaultSyncDto } from "../../ipc/bindings";
import { ConfirmHost } from "../../components/ConfirmHost";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { formatBytes } from "@lunarca/editor-core/format-bytes";
import { formatDate, formatDateTime } from "../../lib/format-datetime";
import { SyncSection } from "./SyncSection";

function vault(overrides: Partial<VaultSyncDto> = {}): VaultSyncDto {
  return {
    path: "/v/work",
    name: "工作",
    is_open: true,
    phase: "off",
    paused: false,
    remote_vault_id: null,
    last_synced_ms: null,
    last_error: null,
    conflict_copies: 0,
    held_deletions: 0,
    too_large: [],
    display_path: "~/v/work",
    local_bytes: 2048,
    cloud_bytes: null,
    bound_account: null,
    ...overrides,
  };
}

const SYNCED = vault({
  path: "/v/read",
  name: "读书",
  is_open: false,
  phase: "idle",
  remote_vault_id: "remote-read",
  display_path: "~/v/read",
  last_synced_ms: Date.UTC(2026, 9, 9, 21, 22, 0),
});

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

const REMOTE = (id: string, name: string): RemoteVaultDto => ({
  id,
  name,
  created_at: "2026-10-09T20:31:22Z",
  bytes_stored: 4096,
});

/**
 * `getByText` 会把 DOM 文本里的空白（含 `formatBytes` 的不换行空格）规整成普通空格，却不规整
 * 匹配串本身：带字节量的期望文案要先做同样的规整。
 */
const plain = (text: string) => text.replace(/\u00a0/g, " ");

function mount() {
  return render(
    withI18n(
      <ConfirmHost>
        <SyncSection />
      </ConfirmHost>,
    ),
  );
}

function switchOf(view: ReturnType<typeof mount>, name: string) {
  return view.getByRole("switch", { name: t("cloud:sync.vault_toggle", { name }) });
}

/** 确认宿主的主按钮（`role=alertdialog` 里最后一颗）。 */
function confirmButton() {
  const buttons = document.body.querySelectorAll<HTMLButtonElement>("[role=alertdialog] button");
  return buttons[buttons.length - 1];
}

beforeEach(() => {
  resetBindingsDouble();
  setCommandReply("syncOverview", { status: "ok", data: overview([vault(), SYNCED]) });
  setCommandReply("syncRemoteVaults", { status: "ok", data: [] });
  // 「这台设备」那几项偏好由 SyncPreferences 自取（它自己的用例覆盖），这里给空表。
  setCommandReply("listSettings", { status: "ok", data: [] });
  for (const name of [
    "syncEnable",
    "syncJoin",
    "syncDisable",
    "syncSetVaultPaused",
    "syncPauseAll",
    "syncResumeAll",
  ]) {
    setCommandReply(name, { status: "ok", data: null });
  }
});
afterEach(cleanup);

describe("云同步一节", () => {
  it.each(["unknown", "not_included", "unlimited"] as const)("容量 %s 由明确状态呈现，不虚构上限", async (kind) => {
    setCommandReply("syncOverview", { status: "ok", data: overview([vault()], {
      storage: { used_bytes: 100, limit: { kind }, alert: "none", unlinked_bytes: 0 },
    }) });
    const view = mount();
    const key = kind === "unlimited" ? "amount_unlimited" : kind === "unknown" ? "amount_unknown" : "amount_not_included";
    await view.findByText(plain(t(`cloud:sync.storage.${key}`, { used: formatBytes(100, "zh-CN") })));
    expect(view.container.querySelector("progress.luna-progress")).toBeNull();
  });

  it("远端列表失败不伪装为空列表；重试成功后才允许新建", async () => {
    setCommandReply("syncRemoteVaults", { status: "error", error: "sync_offline: unavailable" });
    const view = mount();
    fireEvent.click(await view.findByRole("switch", { name: t("cloud:sync.vault_toggle", { name: "工作" }) }));
    await view.findByText(t("cloud:sync.wizard.retry"));
    fireEvent.click(view.getByLabelText(t("cloud:sync.wizard.choice_new")));
    expect(document.body.querySelector("button[type=submit]:disabled")).not.toBeNull();
    setCommandReply("syncRemoteVaults", { status: "ok", data: [] });
    fireEvent.click(view.getByText(t("cloud:sync.wizard.retry")));
    await waitFor(() => expect(document.body.querySelector("button[type=submit]:disabled")).toBeNull());
    expect(commandMock("syncEnable")).not.toHaveBeenCalled();
  });

  it("列表迟到不覆盖已选的新建方案", async () => {
    let resolve!: (value: unknown) => void;
    setCommandReply("syncRemoteVaults", () => new Promise((done) => { resolve = done; }));
    const view = mount();
    fireEvent.click(await view.findByRole("switch", { name: t("cloud:sync.vault_toggle", { name: "工作" }) }));
    const choice = view.getByLabelText(t("cloud:sync.wizard.choice_new")) as HTMLInputElement;
    fireEvent.click(choice);
    await act(async () => resolve({ status: "ok", data: [REMOTE("new", "工作")] }));
    expect(choice.checked).toBe(true);
    expect((view.getByLabelText(/工作（创建于/) as HTMLInputElement).checked).toBe(false);
  });

  it("每个已知库一行一个开关；选中的显示状态与按毫秒格式化的上次同步", async () => {
    const view = mount();
    await waitFor(() => expect(switchOf(view, "工作")).toBeTruthy());
    expect(switchOf(view, "工作").getAttribute("aria-checked")).toBe("false");
    expect(switchOf(view, "读书").getAttribute("aria-checked")).toBe("true");
    expect(view.container.textContent).toContain(
      t("cloud:sync.last_synced", {
        time: formatDateTime(SYNCED.last_synced_ms, "zh-CN"),
      }),
    );
    expect(view.container.textContent).toContain(t("cloud:sync.open_badge"));
  });

  it("打开一个库的开关：向导针对这个库，一步开启，不收任何秘密", async () => {
    const view = mount();
    await waitFor(() => expect(switchOf(view, "工作")).toBeTruthy());
    fireEvent.click(switchOf(view, "工作"));
    expect(await view.findByText(t("cloud:sync.wizard.title", { name: "工作" }))).toBeTruthy();
    expect(document.body.querySelector("input[type=password]")).toBeNull();
    fireEvent.click(await view.findByLabelText(t("cloud:sync.wizard.choice_new")));
    await waitFor(() =>
      expect(document.body.querySelector("button[type=submit]:disabled")).toBeNull(),
    );
    fireEvent.click(document.body.querySelector<HTMLButtonElement>("button[type=submit]")!);
    await waitFor(() => expect(commandMock("syncEnable")).toHaveBeenCalledWith("/v/work", false));
  });

  it("云端列表不替用户选择；明确选择后加入，已绑定库不再列出", async () => {
    setCommandReply("syncRemoteVaults", {
      status: "ok",
      data: [REMOTE("remote-read", "读书"), REMOTE("remote-work", "工作")],
    });
    const view = mount();
    await waitFor(() => expect(switchOf(view, "工作")).toBeTruthy());
    fireEvent.click(switchOf(view, "工作"));
    const joinRadio = (await view.findByLabelText(/工作（创建于/)) as HTMLInputElement;
    expect(joinRadio.checked).toBe(false);
    expect(document.body.querySelector("button[type=submit]:disabled")).not.toBeNull();
    fireEvent.click(joinRadio);
    expect(view.queryByLabelText(/读书（创建于/)).toBeNull();
    fireEvent.click(view.getByText(t("cloud:sync.wizard.join"), { selector: "button" }));
    await waitFor(() =>
      expect(commandMock("syncJoin")).toHaveBeenCalledWith("/v/work", "remote-work", false),
    );
  });

  describe("连在另一个账户下的库", () => {
    const ELSEWHERE = vault({
      path: "/v/home",
      name: "家里",
      is_open: false,
      phase: "other_account",
      bound_account: { label: "jo***@gmail.com", server_host: "10.0.0.62:8443", server_official: false },
      display_path: "~/v/home",
    });

    beforeEach(() => {
      setCommandReply("syncOverview", { status: "ok", data: overview([ELSEWHERE, SYNCED]) });
    });

    it("开关为关，写明连在哪台服务器的哪个账户下，不报错、不给暂停与恢复", async () => {
      const view = mount();
      await waitFor(() => expect(switchOf(view, "家里")).toBeTruthy());
      expect(switchOf(view, "家里").getAttribute("aria-checked")).toBe("false");
      const row = view.container.querySelectorAll("li.cloud-sync__vault")[0];
      const status = row.querySelector(".cloud-sync__status");
      expect(status?.textContent).toBe(
        t("cloud:sync.phase.other_account", { account: "jo***@gmail.com", server: "10.0.0.62:8443" }),
      );
      // 同一个邮箱可能在两台服务器上各有一个账户：只写邮箱就与此刻登录的那个一字不差。
      expect(status?.textContent).toContain("10.0.0.62:8443");
      expect(status?.getAttribute("data-tone")).toBe("muted");
      expect(row.textContent).toContain(t("cloud:sync.other_account"));
      expect(row.querySelector("[data-tone=error]")).toBeNull();
      expect(row.textContent).not.toContain(t("cloud:sync.pause_vault"));
      expect(row.textContent).not.toContain(t("cloud:sync.restore.open"));
      // 不算选中：全部暂停 / 立即同步只看真正选中的那一个（读书）。
      expect(view.getAllByText(t("cloud:sync.pause_vault"))).toHaveLength(1);
    });

    it("改为同步到当前账户：取消确认什么都不发；确认后开向导，开启时带上改连", async () => {
      const view = mount();
      await waitFor(() => expect(switchOf(view, "家里")).toBeTruthy());
      fireEvent.click(view.getByText(t("cloud:sync.take_over.open")));
      expect(await view.findByText(t("cloud:sync.take_over.title", { name: "家里" }))).toBeTruthy();
      expect(view.getByText(t("cloud:sync.take_over.body"))).toBeTruthy();
      // 取消：不开向导、不发命令。
      const buttons = document.body.querySelectorAll<HTMLButtonElement>("[role=alertdialog] button");
      fireEvent.click(buttons[0]);
      await waitFor(() => expect(document.body.querySelector("[role=alertdialog]")).toBeNull());
      expect(view.queryByText(t("cloud:sync.wizard.title", { name: "家里" }))).toBeNull();

      // 开关同样先确认；确认后走同一个向导。
      fireEvent.click(switchOf(view, "家里"));
      await view.findByText(t("cloud:sync.take_over.title", { name: "家里" }));
      fireEvent.click(confirmButton());
      expect(await view.findByText(t("cloud:sync.wizard.title", { name: "家里" }))).toBeTruthy();
      fireEvent.click(await view.findByLabelText(t("cloud:sync.wizard.choice_new")));
      await waitFor(() =>
        expect(document.body.querySelector("button[type=submit]:disabled")).toBeNull(),
      );
      fireEvent.click(document.body.querySelector<HTMLButtonElement>("button[type=submit]")!);
      await waitFor(() => expect(commandMock("syncEnable")).toHaveBeenCalledWith("/v/home", true));
      expect(commandMock("syncDisable")).not.toHaveBeenCalled();
    });

    it("改连被拒的稳定码有文案", () => {
      expect(t("cloud:sync.error.sync_bound_elsewhere")).not.toBe("sync.error.sync_bound_elsewhere");
    });
  });

  it("关掉一个选中库的开关：确认后停止同步（只停这台设备，云端与本地都保留）", async () => {
    const view = mount();
    await waitFor(() => expect(switchOf(view, "读书")).toBeTruthy());
    fireEvent.click(switchOf(view, "读书"));
    expect(
      await view.findByText(t("cloud:sync.disable.title", { name: "读书" })),
    ).toBeTruthy();
    expect(view.getByText(t("cloud:sync.disable.body"))).toBeTruthy();
    expect(t("cloud:sync.disable.body")).toContain("停止在这台设备上同步；本地文件和云端库都保留");
    fireEvent.click(confirmButton());
    await waitFor(() => expect(commandMock("syncDisable")).toHaveBeenCalledWith("/v/read"));
  });

  it("「云端库」只在可用时出现在同步一节里；本机同步的库变了就重取", async () => {
    setCommandReply("syncCloudVaults", { status: "ok", data: [] });
    const view = mount();
    expect(await view.findByText(t("cloud:sync.cloud_vaults.title"))).toBeTruthy();
    await waitFor(() => expect(commandMock("syncCloudVaults")).toHaveBeenCalledTimes(1));
    act(() =>
      emitEvent(
        "syncStatus",
        overview([{ ...vault(), phase: "idle", remote_vault_id: "remote-work" }, SYNCED]),
      ),
    );
    await waitFor(() => expect(commandMock("syncCloudVaults")).toHaveBeenCalledTimes(2));
    act(() => emitEvent("syncStatus", overview([vault()], { gate: "signed_out" })));
    await waitFor(() =>
      expect(view.queryByText(t("cloud:sync.cloud_vaults.title"))).toBeNull(),
    );
  });

  it("同步完成一轮就重取云端库，大小与更新时间不停在旧值", async () => {
    setCommandReply("syncCloudVaults", { status: "ok", data: [] });
    const linked = { ...vault(), phase: "idle" as const, remote_vault_id: "remote-work" };
    const view = mount();
    expect(await view.findByText(t("cloud:sync.cloud_vaults.title"))).toBeTruthy();
    act(() => emitEvent("syncStatus", overview([{ ...linked, last_synced_ms: 1000 }, SYNCED])));
    await waitFor(() => expect(commandMock("syncCloudVaults")).toHaveBeenCalledTimes(2));
    act(() => emitEvent("syncStatus", overview([{ ...linked, last_synced_ms: 2000 }, SYNCED])));
    await waitFor(() => expect(commandMock("syncCloudVaults")).toHaveBeenCalledTimes(3));
  });

  it("单库暂停只动那一行；全部暂停可选时长，暂停中给恢复", async () => {
    const view = mount();
    fireEvent.click(await view.findByText(t("cloud:sync.pause_vault")));
    await waitFor(() =>
      expect(commandMock("syncSetVaultPaused")).toHaveBeenCalledWith("/v/read", true),
    );

    fireEvent.click(view.getByText(t("cloud:sync.pause_all.open")));
    fireEvent.click(view.getByText(t("cloud:sync.pause_all.hours_8")));
    await waitFor(() => expect(commandMock("syncPauseAll")).toHaveBeenCalledWith(8));

    act(() =>
      emitEvent(
        "syncStatus",
        overview([vault(), { ...SYNCED, phase: "paused" }], { paused: { until_ms: null } }),
      ),
    );
    expect(view.getByText(t("cloud:sync.pause_all.paused_indefinitely"))).toBeTruthy();
    fireEvent.click(view.getByText(t("cloud:sync.pause_all.resume")));
    await waitFor(() => expect(commandMock("syncResumeAll")).toHaveBeenCalled());
  });

  it("每个库分层显示：状态标签、大小与时间一行、路径单独一行（悬停看全路径）", async () => {
    setCommandReply("syncOverview", {
      status: "ok",
      data: overview([
        vault({ local_bytes: 2048, display_path: "~/v/work" }),
        { ...SYNCED, local_bytes: 4096, cloud_bytes: 6144, display_path: "~/v/read" },
      ]),
    });
    const view = mount();
    await waitFor(() => expect(switchOf(view, "读书")).toBeTruthy());
    const rows = view.container.querySelectorAll("li.cloud-sync__vault");
    const [work, read] = [rows[0], rows[1]];
    // 状态是带色调的标签，不混在说明文字里。
    expect(work.querySelector(".cloud-sync__status")?.getAttribute("data-tone")).toBe("muted");
    expect(read.querySelector(".cloud-sync__status")?.getAttribute("data-tone")).toBe("success");
    expect(read.querySelector(".cloud-sync__status")?.textContent).toBe(t("cloud:sync.phase.idle"));
    // 没选中的只有本地大小；选中的「本地 · 云端 · 上次同步」。
    // `textContent` 保留不换行空格，直接比原串（`plain` 只给 `getByText` 用）。
    expect(work.querySelector(".cloud-sync__meta")?.textContent).toBe(
      t("cloud:sync.size.local", { local: formatBytes(2048, "zh-CN") }),
    );
    const meta = read.querySelector(".cloud-sync__meta")?.textContent ?? "";
    expect(meta).toContain(t("cloud:sync.size.cloud", { cloud: formatBytes(6144, "zh-CN") }));
    expect(meta).toContain(
      t("cloud:sync.last_synced", { time: formatDateTime(SYNCED.last_synced_ms, "zh-CN") }),
    );
    // 路径单独一行：显示缩写，悬停给全路径。
    const path = read.querySelector(".cloud-sync__path");
    expect(path?.textContent).toBe("~/v/read");
    expect(path?.getAttribute("title")).toBe("/v/read");
    expect(meta).not.toContain("/v/read");
  });

  it("失去同步权益：写明只读与清除云副本的日期和剩余天数，并说本机笔记不受影响", async () => {
    const purge = Date.now() + 10.5 * 86_400_000;
    setCommandReply("syncOverview", {
      status: "ok",
      data: overview([vault(), SYNCED], { read_only: { since_ms: Date.now(), purge_after_ms: purge } }),
    });
    const view = mount();
    const text = plain(t("cloud:sync.read_only_grace", { date: formatDate(purge, "zh-CN"), days: 10 }));
    await waitFor(() => expect(view.getByText(text)).toBeTruthy());
  });

  it("账户容量：有上限画进度与「已用 / 上限」并报快满了；不限不画条；用满给出说明", async () => {
    const storage = { used_bytes: 850, limit: { kind: "limited" as const, bytes: 1000 }, alert: "p80" as const, unlinked_bytes: 300 };
    setCommandReply("syncOverview", { status: "ok", data: overview([vault(), SYNCED], { storage }) });
    const view = mount();
    await waitFor(() => expect(view.getByText(t("cloud:sync.storage.title"))).toBeTruthy());
    expect(view.container.querySelector("progress.luna-progress")?.getAttribute("max")).toBe("1000");
    expect(
      view.getByText(
        plain(
          t("cloud:sync.storage.amount", {
            used: formatBytes(850, "zh-CN"),
            limit: formatBytes(1000, "zh-CN"),
          }),
        ),
      ),
    ).toBeTruthy();
    expect(view.getByText(t("cloud:sync.storage.alert_warning"))).toBeTruthy();
    expect(
      view.getByText(
        plain(t("cloud:sync.storage.desc_unlinked", { size: formatBytes(300, "zh-CN") })),
      ),
    ).toBeTruthy();
    expect(view.queryByText(t("cloud:sync.storage.full"))).toBeNull();

    act(() =>
      emitEvent(
        "syncStatus",
        overview([vault(), { ...SYNCED, phase: "error", last_error: "sync_quota_exceeded" }], {
          // 满不满是服务端那一格说的（`exhausted`），界面不拿已用与上限自己比。
          storage: { ...storage, used_bytes: 1000, alert: "exhausted" },
        }),
      ),
    );
    expect(view.getByText(t("cloud:sync.storage.full"))).toBeTruthy();
    // 用满了徽标说「已满」而不是「即将用完」；那一行写「只读」，与状态栏同一口径。
    expect(view.getByText(t("cloud:sync.storage.alert_full"))).toBeTruthy();
    expect(view.queryByText(t("cloud:sync.storage.alert_critical"))).toBeNull();
    expect(view.container.textContent).toContain(t("cloud:sync.read_only"));

    act(() =>
      emitEvent(
        "syncStatus",
        overview([vault(), SYNCED], {
          storage: { used_bytes: 850, limit: { kind: "unlimited" }, alert: "none", unlinked_bytes: 0 },
        }),
      ),
    );
    expect(view.container.querySelector("progress.luna-progress")).toBeNull();
    expect(
      view.getByText(
        plain(t("cloud:sync.storage.amount_unlimited", { used: formatBytes(850, "zh-CN") })),
      ),
    ).toBeTruthy();
  });

  it("向导先说清容量够不够：本地大小超过剩余空间时给出警示", async () => {
    setCommandReply("syncOverview", {
      status: "ok",
      data: overview([vault({ local_bytes: 2048 }), SYNCED], {
        storage: { used_bytes: 900, limit: { kind: "limited", bytes: 1000 }, alert: "p80", unlinked_bytes: 0 },
      }),
    });
    const view = mount();
    await waitFor(() => expect(switchOf(view, "工作")).toBeTruthy());
    fireEvent.click(switchOf(view, "工作"));
    fireEvent.click(await view.findByLabelText(t("cloud:sync.wizard.choice_new")));
    expect(
      await view.findByText(
        plain(
          t("cloud:sync.wizard.over_capacity", {
            size: formatBytes(2048, "zh-CN"),
            remaining: formatBytes(100, "zh-CN"),
          }),
        ),
      ),
    ).toBeTruthy();
  });

  it("只停了大文件：说出原因与什么时候自动继续", async () => {
    const view = mount();
    await waitFor(() => expect(switchOf(view, "读书")).toBeTruthy());
    act(() => emitEvent("syncStatus", overview([vault(), SYNCED], { large_paused: "metered" })));
    expect(view.getByText(t("cloud:sync.large_paused.metered"))).toBeTruthy();
  });

  it("账户被冻结（纵切 3 C3）：说上传暂停、下载照常、本机改动都留着；解冻后横幅消失", async () => {
    const view = mount();
    await waitFor(() => expect(switchOf(view, "读书")).toBeTruthy());
    act(() => emitEvent("syncStatus", overview([vault(), SYNCED], { account_frozen: true })));
    expect(view.getByText(t("cloud:sync.account_frozen"))).toBeTruthy();
    act(() => emitEvent("syncStatus", overview([vault(), SYNCED])));
    expect(view.queryByText(t("cloud:sync.account_frozen"))).toBeNull();
  });

  it("太大没同步的文件如实列出，其余照常同步", async () => {
    const view = mount();
    await waitFor(() => expect(switchOf(view, "读书")).toBeTruthy());
    act(() =>
      emitEvent(
        "syncStatus",
        overview([vault(), { ...SYNCED, too_large: ["录音/assets/s-p01.wav", "b.mov"] }]),
      ),
    );
    const names = new Intl.ListFormat("zh-CN", { type: "conjunction" }).format(["s-p01.wav", "b.mov"]);
    expect(view.getByText(t("cloud:sync.too_large", { count: 2, names }))).toBeTruthy();
  });

  it("状态跟着事件走：被熔断拦下的删除给出确认入口；认不出的码给通用说法", async () => {
    const view = mount();
    await waitFor(() => expect(switchOf(view, "读书")).toBeTruthy());
    act(() => emitEvent("syncStatus", overview([vault(), { ...SYNCED, held_deletions: 61 }])));
    expect(view.getByText(t("cloud:sync.held_deletions", { count: 61 }))).toBeTruthy();
    expect(view.getByText(t("cloud:sync.confirm_delete.open"))).toBeTruthy();

    act(() =>
      emitEvent(
        "syncStatus",
        overview([vault(), { ...SYNCED, phase: "error", last_error: "brand_new_code" }]),
      ),
    );
    expect(view.getByText(t("cloud:sync.error.other"))).toBeTruthy();
    expect(view.queryByText("brand_new_code")).toBeNull();
  });
});
