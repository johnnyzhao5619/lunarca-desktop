// @vitest-environment happy-dom
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

vi.mock("../../ipc/cloud-bindings", () => import("../../ipc/bindings-test-double"));

import { ConfirmHost } from "../../components/ConfirmHost";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { commandMock, resetBindingsDouble } from "../../ipc/bindings-test-double";
import { WebSessions } from "./WebSessions";

const chrome = { id: "s1", browser: "Chrome", os: "macOS", created_at: "2026-11-08T00:00:00Z", last_seen_at: null };
const unknown = { id: "s2", browser: null, os: null, created_at: "2026-11-07T00:00:00Z", last_seen_at: null };

beforeEach(resetBindingsDouble);
afterEach(cleanup);

it("逐个退出：确认后调用、刷新列表；认不出的浏览器也有名字", async () => {
  const list = commandMock("accountWebSessions")
    .mockResolvedValueOnce({ status: "ok", data: [chrome, unknown] })
    .mockResolvedValueOnce({ status: "ok", data: [unknown] });
  const revoke = commandMock("accountRevokeWebSession").mockResolvedValue({ status: "ok", data: null });
  render(withI18n(<ConfirmHost><WebSessions /></ConfirmHost>));
  expect(await screen.findByText("Chrome")).toBeTruthy();
  expect(screen.getByText(t("cloud:web_sessions.unknown_browser"))).toBeTruthy();

  fireEvent.click(screen.getAllByRole("button", { name: t("cloud:web_sessions.revoke") })[0]);
  const dialog = within(await screen.findByRole("alertdialog", { name: t("cloud:web_sessions.revoke_title", { name: "Chrome" }) }));
  expect(revoke).not.toHaveBeenCalled();
  fireEvent.click(dialog.getByRole("button", { name: t("cloud:web_sessions.revoke") }));

  await vi.waitFor(() => expect(screen.queryByText("Chrome")).toBeNull());
  expect(revoke).toHaveBeenCalledWith("s1");
  expect(list).toHaveBeenCalledTimes(2);
});

it("全部退出失败：列表留着并显示错误", async () => {
  commandMock("accountWebSessions").mockResolvedValue({ status: "ok", data: [chrome] });
  commandMock("accountRevokeAllWebSessions").mockResolvedValue({ status: "error", error: { code: "web_access_unavailable" } });
  render(withI18n(<ConfirmHost><WebSessions /></ConfirmHost>));
  fireEvent.click(await screen.findByRole("button", { name: t("cloud:web_sessions.revoke_all") }));
  const dialog = within(await screen.findByRole("alertdialog", { name: t("cloud:web_sessions.revoke_all_title") }));
  fireEvent.click(dialog.getByRole("button", { name: t("cloud:web_sessions.revoke_all_confirm") }));

  expect((await screen.findByRole("alert")).textContent).toBe(t("cloud:error.web_access_unavailable"));
  expect(screen.getByText("Chrome")).toBeTruthy();
});

it("没有浏览器时写明，且不给「全部退出」", async () => {
  commandMock("accountWebSessions").mockResolvedValue({ status: "ok", data: [] });
  render(withI18n(<ConfirmHost><WebSessions /></ConfirmHost>));
  expect(await screen.findByText(t("cloud:web_sessions.empty"))).toBeTruthy();
  expect(screen.queryByRole("button", { name: t("cloud:web_sessions.revoke_all") })).toBeNull();
});
