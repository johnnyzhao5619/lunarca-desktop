// @vitest-environment happy-dom
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));
import { commandMock, emitEvent, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import type { SettingValueDto } from "../../ipc/bindings";
import { createSettingSpecFixture } from "./settings-test-fixture";
import { useSettingsController } from "./use-settings-controller";
import { useSettingDraft } from "./use-setting-draft";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}
beforeEach(() => {
  resetBindingsDouble();
  setCommandReply("listSettings", { status: "ok", data: [createSettingSpecFixture()] });
  setCommandReply("shortcutDiagnostics", { status: "ok", data: [] });
});
afterEach(cleanup);

it("同一项在途时不重复提交；不同项的保存反馈独立保留", async () => {
  const first = deferred<{ status: "ok"; data: SettingValueDto }>();
  setCommandReply("setSetting", (key: unknown) => key === "a"
    ? first.promise : { status: "ok", data: { kind: "bool", value: true } });
  const { result } = renderHook(() => useSettingsController(true));
  await waitFor(() => expect(result.current.loading).toBe(false));
  let saving!: Promise<SettingValueDto | null>;
  act(() => { saving = result.current.setValue("a", { kind: "bool", value: true }); });
  expect(result.current.pendingKeys.a).toBe(true);
  await act(async () => { expect(await result.current.setValue("a", { kind: "bool", value: false })).toBeNull(); });
  await act(async () => { await result.current.setValue("b", { kind: "bool", value: true }); });
  await act(async () => { first.resolve({ status: "ok", data: { kind: "bool", value: true } }); await saving; });
  expect(commandMock("setSetting")).toHaveBeenCalledTimes(2);
  expect(result.current.savedKeys).toEqual({ a: true, b: true });
  expect(result.current.pendingKeys).toEqual({});
});

it("较早发起的读取迟到时不覆盖较新设置快照", async () => {
  const old = deferred<unknown>();
  const { result } = renderHook(() => useSettingsController(true));
  await waitFor(() => expect(result.current.loading).toBe(false));
  setCommandReply("listSettings", () => old.promise);
  act(() => { emitEvent("settingsChanged", { key: "privacy.strict_local" }); });
  setCommandReply("listSettings", { status: "ok", data: [createSettingSpecFixture({ current_value: { kind: "bool", value: true } })] });
  await act(async () => { await result.current.refresh(); });
  await act(async () => { old.resolve({ status: "ok", data: [createSettingSpecFixture()] }); });
  expect(result.current.specs[0].current_value.value).toBe(true);
});

it("失败输入跨刷新保留；旧回执不覆盖提交后的新输入", () => {
  const { result, rerender } = renderHook(({ current }) => useSettingDraft(current), { initialProps: { current: "original" } });
  act(() => result.current.setDraft("invalid"));
  const submittedVersion = result.current.version.current;
  rerender({ current: "external update" });
  expect(result.current.draft).toBe("invalid");
  act(() => result.current.setDraft("new edit"));
  act(() => result.current.adopt("normalized old edit", submittedVersion));
  expect(result.current.draft).toBe("new edit");
  act(() => result.current.adopt("normalized new edit", result.current.version.current));
  rerender({ current: "fresh persisted value" });
  expect(result.current.draft).toBe("fresh persisted value");
});

it("过期重置计划保持错误可见且不清除草稿；成功才更新恢复次数", async () => {
  const { result } = renderHook(() => useSettingsController(true));
  await waitFor(() => expect(result.current.loading).toBe(false));
  setCommandReply("resetSettingGroup", { status: "error", error: "SETTING_RESET_STALE: changed" });
  await act(async () => { expect(await result.current.resetPage("privacy", "standard", "old")).toBeNull(); });
  expect(result.current.error).toBe("SETTING_RESET_STALE");
  expect(result.current.resetEpochs).toEqual({});
  setCommandReply("resetSettingGroup", { status: "ok", data: 1 });
  await act(async () => { await result.current.resetPage("privacy", "standard", "new"); });
  expect(commandMock("resetSettingGroup")).toHaveBeenLastCalledWith("privacy", "standard", "new");
  expect(result.current.resetEpochs["privacy.strict_local"]).toBe(1);
});

it("注册表读回之前不挑首页：先挂载、后进入设置也落在第一页而不是面板型页", async () => {
  // 工作区一挂载控制器就在（`enabled=false`，不取数），此时页面清单只剩面板型页。
  const { result, rerender } = renderHook(({ enabled }) => useSettingsController(enabled), {
    initialProps: { enabled: false },
  });
  expect(result.current.activePage).toBe("");
  setCommandReply("listSettings", {
    status: "ok",
    data: [createSettingSpecFixture({ key: "general.language", group: "general" })],
  });
  rerender({ enabled: true });
  await waitFor(() => expect(result.current.loading).toBe(false));
  expect(result.current.activePage).toBe("general");
});

it("注册表一次都没读成：不落到面板型页，错误留给视图如实显示", async () => {
  setCommandReply("listSettings", { status: "error", error: "VAULT_IO: x" });
  const { result } = renderHook(() => useSettingsController(true));
  await waitFor(() => expect(result.current.loading).toBe(false));
  expect(result.current.activePage).toBe("");
  expect(result.current.error).not.toBeNull();
});
