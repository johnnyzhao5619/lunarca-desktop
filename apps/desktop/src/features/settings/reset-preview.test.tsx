// @vitest-environment happy-dom
import { cleanup, fireEvent, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));
import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { ConfirmHost } from "../../components/ConfirmHost";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { createSettingSpecFixture, createSettingsControllerFixture } from "./settings-test-fixture";
import { useResetSettingsPage } from "./use-reset-settings-page";

const resetPage = vi.fn(() => Promise.resolve(1));
function Entry() {
  const reset = useResetSettingsPage(createSettingsControllerFixture({ activePage: "privacy", resetPage }));
  return <button onClick={() => void reset("privacy")}>{t("settings:action.reset")}</button>;
}
beforeEach(() => { resetBindingsDouble(); resetPage.mockClear(); });
afterEach(cleanup);
const mount = () => render(withI18n(<ConfirmHost><Entry /></ConfirmHost>));

it("确认框展示 Rust 计划的当前值与默认值，取消没有写入", async () => {
  setCommandReply("previewSettingGroupReset", { status: "ok", data: { fingerprint: "plan", changes: [createSettingSpecFixture({ current_value: { kind: "bool", value: true }, modified: true })] } });
  const view = mount();
  fireEvent.click(view.getByText(t("settings:action.reset")));
  const dialog = await view.findByRole("alertdialog");
  expect(dialog.querySelector(".luna-settings-panel__row-value")?.textContent?.split(" → ")).toEqual([
    t("settings:action.value_enabled"), t("settings:action.value_disabled"),
  ]);
  fireEvent.click(view.getByText(t("common:action.cancel")));
  await waitFor(() => expect(view.queryByRole("alertdialog")).toBeNull());
  expect(resetPage).not.toHaveBeenCalled();
});

it("确认传回预览指纹与范围，不在前端重算待改的键", async () => {
  setCommandReply("previewSettingGroupReset", { status: "ok", data: { fingerprint: "sealed", changes: [createSettingSpecFixture({ modified: true })] } });
  const view = mount();
  fireEvent.click(view.getByText(t("settings:action.reset")));
  fireEvent.click(await view.findByText(t("settings:action.reset_group_standard"), { selector: "button" }));
  await waitFor(() => expect(resetPage).toHaveBeenCalledWith("privacy", "standard", "sealed"));
  expect(commandMock("previewSettingGroupReset")).toHaveBeenCalledWith("privacy", "standard");
});

it("预览失败只显示关闭入口，不执行恢复", async () => {
  setCommandReply("previewSettingGroupReset", { status: "error", error: "unavailable" });
  const view = mount();
  fireEvent.click(view.getByText(t("settings:action.reset")));
  fireEvent.click(await view.findByText(t("settings:action.close"), { selector: "button" }));
  expect(resetPage).not.toHaveBeenCalled();
});
