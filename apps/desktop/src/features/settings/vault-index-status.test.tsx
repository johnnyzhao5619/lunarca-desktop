// @vitest-environment happy-dom
/**
 * 索引扫描现状（PRD-06-004 ②④，笔记库对标轮 V1）的真实渲染守护。
 *
 * - 开库那一轮在窗口挂监听前就开始了：只听事件会把进行中的扫描看成「没在扫」，搜索与
 *   快速切换器就会拿不全的结果冒充「没有」。首帧必须来自 `vault_index_status`；
 * - 事件先到时，晚回来的快照不能盖掉更新的进度；
 * - 设置页在扫描中给「取消」，取消的终态说「已取消」，不说「已更新」。
 */
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import {
  commandMock,
  emitEvent,
  resetBindingsDouble,
  setCommandReply,
} from "../../ipc/bindings-test-double";
import { useIndexStatus } from "../../hooks/use-index-status";
import { withI18n } from "../../i18n/i18n-test-fixture";
import { VaultIndexPanel } from "./VaultIndexPanel";

function Probe() {
  const status = useIndexStatus();
  return <output data-status={status ? `${status.indexed}/${status.total}` : "none"} />;
}

const flush = () => act(async () => {});

afterEach(() => {
  cleanup();
  resetBindingsDouble();
});

describe("useIndexStatus", () => {
  it("挂载前已开始的扫描从现状查询拿到，终态帧收起", async () => {
    setCommandReply("vaultIndexStatus", { scanning: true, indexed: 128, total: 900 });
    const view = render(<Probe />);
    await flush();
    expect(view.container.querySelector("output")?.dataset.status).toBe("128/900");
    act(() => {
      emitEvent("scanProgress", {
        indexed: 900,
        total: 900,
        current_path: null,
        done: true,
        cancelled: false,
      });
    });
    expect(view.container.querySelector("output")?.dataset.status).toBe("none");
  });

  it("事件先到时晚回的快照不覆盖", async () => {
    let resolve: (value: unknown) => void = () => {};
    setCommandReply("vaultIndexStatus", () => new Promise((r) => (resolve = r)));
    const view = render(<Probe />);
    act(() => {
      emitEvent("scanProgress", {
        indexed: 256,
        total: 900,
        current_path: "a.md",
        done: false,
        cancelled: false,
      });
    });
    resolve({ scanning: true, indexed: 0, total: 900 });
    await flush();
    expect(view.container.querySelector("output")?.dataset.status).toBe("256/900");
  });

  it("什么都没变（total = 0）不算扫描中", async () => {
    setCommandReply("vaultIndexStatus", { scanning: true, indexed: 0, total: 0 });
    const view = render(<Probe />);
    await flush();
    expect(view.container.querySelector("output")?.dataset.status).toBe("none");
  });
});

describe("VaultIndexPanel", () => {
  it("扫描中给取消，取消后的终态说已取消", async () => {
    setCommandReply("vaultIndexStatus", { scanning: true, indexed: 128, total: 900 });
    const view = render(withI18n(<VaultIndexPanel />));
    await flush();
    expect(view.getByRole("status").textContent).toContain("128 / 900");
    act(() => {
      view.getByRole("button", { name: "取消" }).click();
    });
    expect(commandMock("cancelVaultScan")).toHaveBeenCalledTimes(1);
    act(() => {
      emitEvent("scanProgress", {
        indexed: 256,
        total: 900,
        current_path: null,
        done: true,
        cancelled: true,
      });
    });
    expect(view.getByRole("status").textContent).toContain("已取消");
    expect(view.getByRole("button", { name: "重建索引" })).toBeDefined();
  });
});
