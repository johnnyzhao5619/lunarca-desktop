// @vitest-environment happy-dom
/**
 * 关于与更新（PRD-09-051，客户端分发 D2）。钉这些事：
 * 1. 版本号来自 Rust 的 app_info；
 * 2. 检查更新三种结局都有字：已是最新 / 有新版本（带发布说明与安装按钮）/ 失败按稳定错误码给原因；
 * 3. 严格本地的错误码是大写的 STRICT_LOCAL_BLOCKED，照样认得；
 * 4. 每个错误码的文案中英都在（缺了就是界面上一行裸 key）；
 * 5. 检查更新是 opt-in 联网：提问卡显不显示只听 Rust，答了就消失、把答案原样交给 Rust。
 */

import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import en from "../../i18n/locales/en-US/settings.json";
import zh from "../../i18n/locales/zh-CN/settings.json";
import { AppReleasePanel, RELEASE_ERROR_CODES, releaseErrorCode } from "./AppReleasePanel";

const release = {
  version: "1.2.0",
  current_version: "1.1.0",
  notes: "修好了同步卡顿\n新增 Google 登录",
  date: null,
};

function mount() {
  setCommandReply("appInfo", { version: "1.1.0" });
  return render(withI18n(createElement(AppReleasePanel)));
}

beforeEach(resetBindingsDouble);
afterEach(cleanup);

describe("关于与更新", () => {
  it("显示当前版本；检查后已是最新", async () => {
    setCommandReply("appReleaseCheck", { status: "ok", data: null });
    mount();
    expect(await screen.findByText(t("settings:app_release.version", { version: "1.1.0" }))).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: t("settings:app_release.check") }));
    expect(await screen.findByText(t("settings:app_release.up_to_date"))).toBeTruthy();
  });

  it("有新版本：显示版本与发布说明，安装失败如实说明且保留可重试", async () => {
    setCommandReply("appReleaseCheck", { status: "ok", data: release });
    setCommandReply("appReleaseInstall", { status: "error", error: "update_install_failed" });
    mount();
    fireEvent.click(screen.getByRole("button", { name: t("settings:app_release.check") }));
    expect(await screen.findByText(t("settings:app_release.available", { version: "1.2.0" }))).toBeTruthy();
    expect(screen.getByText("新增 Google 登录")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: t("settings:app_release.install") }));
    await waitFor(() => expect(commandMock("appReleaseInstall")).toHaveBeenCalled());
    expect(await screen.findByText(t("settings:app_release.error.update_install_failed"))).toBeTruthy();
  });

  it("严格本地拦下检查：大写错误码也认得", async () => {
    setCommandReply("appReleaseCheck", { status: "error", error: "STRICT_LOCAL_BLOCKED" });
    mount();
    fireEvent.click(screen.getByRole("button", { name: t("settings:app_release.check") }));
    expect(await screen.findByText(t("settings:app_release.error.strict_local_blocked"))).toBeTruthy();
    expect(releaseErrorCode("something_else")).toBe("unknown");
  });

  it("提问卡：Rust 说要问才显示，点「启动时检查」把 true 交给 Rust，答完消失", async () => {
    setCommandReply("appReleaseStartupCheckPrompt", true);
    setCommandReply("appReleaseAnswerStartupCheck", { status: "ok", data: null });
    mount();
    expect(await screen.findByText(t("settings:app_release.ask_title"))).toBeTruthy();
    setCommandReply("appReleaseStartupCheckPrompt", false);
    fireEvent.click(screen.getByRole("button", { name: t("settings:app_release.ask_enable") }));
    await waitFor(() => expect(commandMock("appReleaseAnswerStartupCheck")).toHaveBeenCalledWith(true));
    await waitFor(() => expect(screen.queryByText(t("settings:app_release.ask_title"))).toBeNull());
  });

  it("提问卡：Rust 说不问就不出现", async () => {
    setCommandReply("appReleaseStartupCheckPrompt", false);
    mount();
    expect(await screen.findByText(t("settings:app_release.version", { version: "1.1.0" }))).toBeTruthy();
    expect(screen.queryByText(t("settings:app_release.ask_title"))).toBeNull();
  });

  it("每个错误码的文案中英都在", () => {
    for (const code of [...RELEASE_ERROR_CODES, "unknown"]) {
      expect(zh.app_release.error[code as keyof typeof zh.app_release.error], code).toBeTruthy();
      expect(en.app_release.error[code as keyof typeof en.app_release.error], code).toBeTruthy();
    }
  });
});
