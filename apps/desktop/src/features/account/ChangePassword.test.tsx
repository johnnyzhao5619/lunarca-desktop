// @vitest-environment happy-dom
/** 登录状态下改密码（ADR-158 ⑤）：两次输入不一致不发；一致才把新密码交给 Rust，成功后收起并说结果。 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));
vi.mock("../../ipc/cloud-bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble } from "../../ipc/bindings-test-double";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { ChangePassword } from "./ChangePassword";

beforeEach(resetBindingsDouble);
afterEach(cleanup);

describe("账户 · 改密码", () => {
  it("不一致不发；一致才发，成功后收起并说其他设备已退出", async () => {
    render(withI18n(<ChangePassword />));
    fireEvent.click(screen.getByRole("button", { name: t("cloud:password.change_open") }));
    fireEvent.change(screen.getByLabelText(t("cloud:password.new_password")), { target: { value: "new secret 1" } });
    fireEvent.change(screen.getByLabelText(t("cloud:password.confirm_password")), { target: { value: "new secret 2" } });
    fireEvent.click(screen.getByRole("button", { name: t("cloud:password.change_submit") }));
    expect(await screen.findByText(t("cloud:password.mismatch"))).toBeTruthy();
    expect(commandMock("accountChangePassword")).not.toHaveBeenCalled();

    fireEvent.change(screen.getByLabelText(t("cloud:password.confirm_password")), { target: { value: "new secret 1" } });
    fireEvent.click(screen.getByRole("button", { name: t("cloud:password.change_submit") }));
    await waitFor(() => expect(commandMock("accountChangePassword")).toHaveBeenCalledWith("new secret 1"));
    expect(await screen.findByText(t("cloud:password.changed"))).toBeTruthy();
    expect(screen.queryByLabelText(t("cloud:password.new_password"))).toBeNull();
  });
});
