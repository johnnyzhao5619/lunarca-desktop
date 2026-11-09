// @vitest-environment happy-dom
/**
 * 登录后的一屏（ADR-178 ② ④）：Rust 说要问才弹；云端有库时预选合并并给预览；放不下时写明录音留在本机；
 * 「暂不同步」记下决定、不开同步。
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, waitFor } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));
vi.mock("../../hooks/use-account", () => ({
  useAccount: () => ({ phase: { phase: "signed_in", profile: { email: "a@b.c" } } }),
}));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { FirstSyncPrompt } from "./FirstSyncPrompt";

const OFFER = { vault_path: "/v", vault_name: "Notes", local_bytes: 500, recording_bytes: 400 };
const overview = (remaining: number) => ({
  vaults: [],
  storage: { limit: { kind: "limited", bytes: remaining }, used_bytes: 0 },
});

function show() {
  return render(withI18n(<FirstSyncPrompt />));
}

beforeEach(() => {
  resetBindingsDouble();
  setCommandReply("syncFirstLoginOffer", { status: "ok", data: OFFER });
  setCommandReply("syncOverview", { status: "ok", data: overview(1000) });
  setCommandReply("syncRemoteVaults", { status: "ok", data: [] });
  setCommandReply("syncFirstLoginDecided", { status: "ok", data: null });
  setCommandReply("syncMergePreview", {
    status: "ok",
    data: { download: 3, upload: 2, upload_bytes: 100, same: 1, differ: 1 },
  });
});

afterEach(cleanup);

describe("FirstSyncPrompt", () => {
  it("Rust 说不用问：什么都不弹", async () => {
    setCommandReply("syncFirstLoginOffer", { status: "ok", data: null });
    const view = show();
    await act(async () => {});
    expect(view.baseElement.querySelector("[role=dialog]")).toBeNull();
  });

  it("云端没有库：预选新建，暂不同步会记下决定且不开同步", async () => {
    const view = show();
    const notNow = await view.findByRole("button", { name: t("cloud:sync.wizard.not_now") });
    await waitFor(() =>
      expect((view.getByRole("radio", { name: t("cloud:sync.wizard.choice_new") }) as HTMLInputElement).checked).toBe(true),
    );
    fireEvent.click(notNow);
    await waitFor(() => expect(commandMock("syncFirstLoginDecided")).toHaveBeenCalledWith("/v"));
    expect(commandMock("syncEnable")).not.toHaveBeenCalled();
  });

  it("云端有库：预选合并、给预览", async () => {
    setCommandReply("syncRemoteVaults", {
      status: "ok",
      data: [{ id: "r1", name: "Cloud", created_at: "2026-11-01T00:00:00Z", bytes_stored: 10 }],
    });
    const view = show();
    await waitFor(() => expect(commandMock("syncMergePreview")).toHaveBeenCalledWith("/v", "r1"));
    await view.findByText(/\b3\b.*\b2\b/u);
  });

  it("放不下：写明录音留在本机，确认时改成不上传录音", async () => {
    setCommandReply("syncOverview", { status: "ok", data: overview(200) });
    setCommandReply("setSetting", { status: "ok", data: null });
    setCommandReply("syncEnable", { status: "ok", data: null });
    const view = show();
    await view.findByText(t("cloud:sync.wizard.recordings_stay"));
    const start = view.getByRole("button", { name: t("cloud:sync.wizard.create") });
    await waitFor(() => expect((start as HTMLButtonElement).disabled).toBe(false));
    fireEvent.click(start);
    await waitFor(() => expect(commandMock("syncEnable")).toHaveBeenCalled());
    expect(commandMock("setSetting")).toHaveBeenCalledWith("sync.recordings_mode", { kind: "str", value: "off" });
  });
});
