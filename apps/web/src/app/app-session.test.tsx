// @vitest-environment happy-dom
/**
 * 会话状态机（TDD-17 §4）：登出与锁定都终止持有秘密的 Worker、换一个新的，主线程不留旧 Worker 的引用；
 * 锁定后屏幕上不留正文，会话被挤掉时登录页写明原因。
 */

import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { I18nextProvider } from "react-i18next";
import i18n, { storedLanguage } from "../i18n/i18n";
import type { CommandName, WorkerEvent, WorkerMessage, WorkerRequest } from "../worker/protocol";
import { WorkerClient, type WorkerLike } from "../worker/worker-client";
import { App } from "./App";

const SESSION = { session_id: "s", keep_signed_in: true, account: { email: "a@example.com", has_password: true, providers: [] } };

type Script = Partial<Record<CommandName, unknown>>;

class ScriptedWorker implements WorkerLike {
  onmessage: ((message: MessageEvent<WorkerMessage>) => void) | null = null;
  readonly received: CommandName[] = [];
  terminated = false;
  constructor(private readonly script: Script) {}
  postMessage(request: WorkerRequest): void {
    this.received.push(request.cmd);
    const value = this.script[request.cmd];
    queueMicrotask(() => this.onmessage?.(new MessageEvent("message", { data: { id: request.id, ok: true, value } })));
  }
  terminate(): void {
    this.terminated = true;
  }
  emit(event: WorkerEvent): void {
    this.onmessage?.(new MessageEvent("message", { data: event }));
  }
}

function setup(scripts: Script[]) {
  const workers: ScriptedWorker[] = [];
  const spawn = vi.fn(() => {
    const worker = new ScriptedWorker(scripts[Math.min(workers.length, scripts.length - 1)]);
    workers.push(worker);
    return new WorkerClient(worker);
  });
  render(
    <I18nextProvider i18n={i18n}>
      <App spawn={spawn} />
    </I18nextProvider>,
  );
  return workers;
}

const READY: Script = {
  restore: { state: "signed_in", session: SESSION },
  list_vaults: [],
  sync_entitled: true,
  create_default_vault: null,
  logout: null,
  info: { locked: true, session: SESSION },
};

afterEach(cleanup);

describe("会话状态机", () => {
  it("未登录：显示官网与下载链接", async () => {
    setup([{ restore: { state: "signed_out", reason: null } }]);
    await screen.findByRole("heading", { name: i18n.t("web:signin.title") });
    const header = within(screen.getByRole("banner"));
    expect(header.getByRole("link", { name: i18n.t("web:signin.home") }).getAttribute("href")).toBe("https://lunarca.app/");
    expect(header.getByRole("link", { name: i18n.t("web:signin.downloadNav") }).getAttribute("href")).toBe(i18n.t("web:signin.downloadUrl"));
  });

  it("注册：邮箱、验证码、口令，除「保持登录」外没有要勾的项，完成后直接进入", async () => {
    const workers = setup([
      {
        restore: { state: "signed_out", reason: null },
        signup_start: null,
        signup_verify: null,
        signup_complete: SESSION,
        list_vaults: [],
        sync_entitled: true,
        create_default_vault: null,
      },
    ]);
    fireEvent.click(await screen.findByRole("button", { name: i18n.t("web:signin.signUp") }));
    fireEvent.change(await screen.findByLabelText(i18n.t("web:signin.email")), { target: { value: "new@example.com" } });
    fireEvent.click(screen.getByRole("button", { name: i18n.t("web:signup.sendCode") }));
    fireEvent.change(await screen.findByLabelText(i18n.t("web:signup.code")), { target: { value: "123456" } });
    fireEvent.click(screen.getByRole("button", { name: i18n.t("web:signup.verify") }));
    fireEvent.change(await screen.findByLabelText(i18n.t("web:signup.password")), { target: { value: "long enough" } });
    const create = screen.getByRole("button", { name: i18n.t("web:signup.create") }) as HTMLButtonElement;
    expect(screen.getAllByRole("checkbox")).toHaveLength(1);
    expect(create.disabled).toBe(false);
    fireEvent.click(create);
    expect(await screen.findByTestId("no-vaults")).toBeTruthy();
    expect(workers[0].received).toContain("signup_complete");
  });

  it("未登录页切换语言：立即换文案并记在浏览器里，下次打开沿用", async () => {
    setup([{ restore: { state: "signed_out", reason: null } }]);
    const select = await screen.findByRole("combobox", { name: i18n.t("web:signin.language") });
    fireEvent.change(select, { target: { value: "en-US" } });
    await screen.findByRole("heading", { name: i18n.getFixedT("en-US", "web")("signin.title") });
    expect(storedLanguage(window.localStorage)).toBe("en-US");
    fireEvent.change(screen.getByRole("combobox"), { target: { value: "zh-CN" } });
    await screen.findByRole("heading", { name: i18n.getFixedT("zh-CN", "web")("signin.title") });
    window.localStorage.clear();
  });

  it("登出：调登出、终止这个 Worker、换新的，回登录页", async () => {
    const workers = setup([READY, { restore: { state: "signed_out", reason: null } }]);
    expect(await screen.findByTestId("no-vaults")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: i18n.t("web:header.signOut") }));
    expect(await screen.findByRole("heading", { name: i18n.t("web:signin.title") })).toBeTruthy();
    expect(workers[0].received).toContain("logout");
    expect(workers[0].terminated).toBe(true);
    expect(workers[0].onmessage).toBeNull();
    expect(workers).toHaveLength(2);
    expect(workers[1].terminated).toBe(false);
  });

  it("被锁（Worker 报 session_locked）：终止旧 Worker，新 Worker 只取 session/info，屏幕上不留正文", async () => {
    const workers = setup([READY, { info: { locked: true, session: SESSION } }]);
    expect(await screen.findByTestId("no-vaults")).toBeTruthy();
    await act(async () => workers[0].emit({ event: "session_locked" }));
    expect(await screen.findByTestId("lock-screen")).toBeTruthy();
    expect(screen.queryByRole("link", { name: i18n.t("web:signin.home") })).toBeNull();
    expect(workers[0].terminated).toBe(true);
    expect(workers[1].received).toEqual(["info"]);
    expect(screen.queryByTestId("no-vaults")).toBeNull();
  });

  it("在本浏览器「退出全部网页会话」：确认卡（不是 window.confirm）确认后当场本地退出，登录页不写「在别处被吊销」", async () => {
    const workers = setup([
      {
        ...READY,
        get_web_access: { enabled: true },
        list_sessions: { sessions: [{ id: "s", browser: "Chrome", os: "macOS", keep_signed_in: true, created_at: "2026-11-06T00:00:00Z", last_seen_at: null, current: true }], max: 5 },
        revoke_all_sessions: { revoked: 1 },
      },
      { restore: { state: "signed_out", reason: null } },
    ]);
    expect(await screen.findByTestId("no-vaults")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: i18n.t("web:header.account") }));
    const revokeAll = await screen.findByRole("button", { name: i18n.t("web:account.revokeAll") });
    await screen.findByText("Chrome · macOS", { exact: false });
    fireEvent.click(revokeAll);
    const dialog = await screen.findByRole("alertdialog", { name: i18n.t("web:account.revokeAllConfirm") });
    expect(workers[0].received).not.toContain("revoke_all_sessions");
    fireEvent.click(within(dialog).getByRole("button", { name: i18n.t("web:account.revokeAll") }));
    expect(await screen.findByRole("heading", { name: i18n.t("web:signin.title") })).toBeTruthy();
    expect(screen.queryByTestId("signin-notice")).toBeNull();
    expect(workers[0].received).toEqual(expect.arrayContaining(["revoke_all_sessions", "logout"]));
    expect(workers[0].terminated).toBe(true);
  });

  it("确认卡：Esc / 取消都不执行，焦点先落在「取消」上", async () => {
    const workers = setup([
      {
        ...READY,
        get_web_access: { enabled: true },
        list_sessions: { sessions: [], max: 5 },
      },
    ]);
    expect(await screen.findByTestId("no-vaults")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: i18n.t("web:header.account") }));
    fireEvent.click(await screen.findByRole("button", { name: i18n.t("web:account.revokeAll") }));
    const dialog = await screen.findByRole("alertdialog");
    const cancel = within(dialog).getByRole("button", { name: i18n.t("web:common.cancel") });
    expect(document.activeElement).toBe(cancel);
    fireEvent.click(cancel);
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(workers[0].received).not.toContain("revoke_all_sessions");
  });

  it("会话被挤掉：终止 Worker，登录页写明原因", async () => {
    const workers = setup([READY, {}]);
    expect(await screen.findByTestId("no-vaults")).toBeTruthy();
    await act(async () => workers[0].emit({ event: "session_ended", reason: "web_session_displaced" }));
    expect((await screen.findByTestId("signin-notice")).textContent).toBe(i18n.t("web:ended.web_session_displaced"));
    expect(workers[0].terminated).toBe(true);
  });
});
