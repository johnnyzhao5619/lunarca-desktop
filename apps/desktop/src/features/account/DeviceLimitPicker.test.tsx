// @vitest-environment happy-dom
/**
 * 登录撞上设备上限（纵切 3 C5，PRD-13-021）。
 *
 * 钉五件事：
 * 1. 「已达到设备上限（N 台）」+ 每台的名称、平台、最近使用 + 每台一颗「移除这台并登录」；
 * 2. 点下去只交回设备 id（撤销与重发是 Rust 的一条命令），在途时所有按钮不可点；
 * 3. 又撞上限 → 就地说「再选一台」；连不上 / 限频 → 就地说原因、选择还在；
 * 4. 令牌失效 → 交给登录流程去说（本组件随账户状态离场）；
 * 5. 登录流程在账户状态为 `device_limit` 时渲染这一步——无论是哪一种登录方式撞上的。
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));
vi.mock("../../ipc/cloud-bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import type { AccountStateDto, DeviceLimitDto } from "../../ipc/cloud-bindings";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { DeviceLimitPicker } from "./DeviceLimitPicker";
import { SignInFlow } from "./SignIn";

const LIMIT: DeviceLimitDto = {
  max: 2,
  devices: [
    { id: "d1", name: "Mac mini", platform: "macos", last_seen_at: "2026-10-01T08:00:00Z" },
    { id: "d2", name: "Old PC", platform: "windows", last_seen_at: null },
  ],
  expires_at: "2026-10-14T12:10:00Z",
};

const removeLabel = () => t("cloud:device_limit.remove");

beforeEach(() => resetBindingsDouble());
afterEach(cleanup);

describe("设备上限 · 选一台移除", () => {
  it("说清上限，逐台列出名称、平台、最近使用，每台一颗按钮", () => {
    render(withI18n(<DeviceLimitPicker limit={LIMIT} onExit={vi.fn()} />));
    expect(screen.getByRole("heading", { name: t("cloud:device_limit.title", { max: 2 }) })).toBeTruthy();
    expect(screen.getByText("Mac mini")).toBeTruthy();
    expect(screen.getByText("Old PC")).toBeTruthy();
    expect(screen.getByText(t("cloud:devices.never_seen"), { exact: false })).toBeTruthy();
    expect(screen.getByText(t("cloud:devices.platform.windows"), { exact: false })).toBeTruthy();
    expect(screen.getAllByRole("button", { name: removeLabel() })).toHaveLength(2);
    // 诚实边界：被移除的设备本地数据不删。
    expect(screen.getByText(t("cloud:device_limit.note"))).toBeTruthy();
  });

  it("点下去只交回那台的 id；在途时全部按钮不可点；成功后重取账户状态", async () => {
    let finish: (value: unknown) => void = () => {};
    setCommandReply(
      "accountSignInResolveDeviceLimit",
      () => new Promise((resolve) => (finish = resolve)),
    );
    render(withI18n(<DeviceLimitPicker limit={LIMIT} onExit={vi.fn()} />));
    fireEvent.click(screen.getAllByRole("button", { name: removeLabel() })[1]);
    expect(commandMock("accountSignInResolveDeviceLimit")).toHaveBeenCalledWith("d2");
    for (const button of screen.getAllByRole("button")) {
      expect((button as HTMLButtonElement).disabled).toBe(true);
    }
    finish({ status: "ok", data: { outcome: "signed_in" } });
    await waitFor(() => expect(commandMock("accountState")).toHaveBeenCalled());
  });

  it("又撞上限：就地说再选一台", async () => {
    setCommandReply("accountSignInResolveDeviceLimit", {
      status: "ok",
      data: { outcome: "device_limit_reached" },
    });
    render(withI18n(<DeviceLimitPicker limit={LIMIT} onExit={vi.fn()} />));
    fireEvent.click(screen.getAllByRole("button", { name: removeLabel() })[0]);
    await screen.findByText(t("cloud:device_limit.again"));
  });

  it("连不上 / 限频：选择还在，就地说原因，不离场", async () => {
    const onExit = vi.fn();
    setCommandReply("accountSignInResolveDeviceLimit", {
      status: "error",
      error: { code: "rate_limited", retry_after_secs: null },
    });
    render(withI18n(<DeviceLimitPicker limit={LIMIT} onExit={onExit} />));
    fireEvent.click(screen.getAllByRole("button", { name: removeLabel() })[0]);
    await screen.findByText(t("cloud:error.rate_limited"));
    expect(onExit).not.toHaveBeenCalled();
    expect((screen.getAllByRole("button", { name: removeLabel() })[0] as HTMLButtonElement).disabled).toBe(false);
  });

  it("令牌失效：交给登录流程说「重新登录」", async () => {
    const onExit = vi.fn();
    setCommandReply("accountSignInResolveDeviceLimit", {
      status: "error",
      error: { code: "device_limit_expired" },
    });
    render(withI18n(<DeviceLimitPicker limit={LIMIT} onExit={onExit} />));
    fireEvent.click(screen.getAllByRole("button", { name: removeLabel() })[0]);
    await waitFor(() =>
      expect(onExit).toHaveBeenCalledWith({ kind: "error", error: { code: "device_limit_expired" } }),
    );
  });

  it("取消：丢掉这次登录", async () => {
    render(withI18n(<DeviceLimitPicker limit={LIMIT} onExit={vi.fn()} />));
    fireEvent.click(screen.getByRole("button", { name: t("cloud:device_limit.cancel") }));
    await waitFor(() => expect(commandMock("accountSignInCancel")).toHaveBeenCalled());
  });
});

describe("设备上限 · 登录流程", () => {
  const server = { origin: "https://cloud.example", display_name: null, kind: "official" } as const;

  it("账户状态是 device_limit：渲染选设备那一步（不是等浏览器、也不是表单）", () => {
    const account: AccountStateDto = { server, phase: { phase: "device_limit", limit: LIMIT }, incoming_link: null };
    render(withI18n(<SignInFlow account={account} embedded={false} />));
    expect(screen.getByText(t("cloud:device_limit.title", { max: 2 }))).toBeTruthy();
    expect(screen.queryByText(t("cloud:flow.waiting"))).toBeNull();
    expect(document.querySelector("input")).toBeNull();
  });

  it("令牌失效后回到登录：说清要重新登录", async () => {
    setCommandReply("accountSignInResolveDeviceLimit", {
      status: "error",
      error: { code: "device_limit_expired" },
    });
    const limited: AccountStateDto = { server, phase: { phase: "device_limit", limit: LIMIT }, incoming_link: null };
    const { rerender } = render(withI18n(<SignInFlow account={limited} embedded={false} />));
    fireEvent.click(screen.getAllByRole("button", { name: removeLabel() })[0]);
    await waitFor(() => expect(commandMock("accountSignInResolveDeviceLimit")).toHaveBeenCalled());
    rerender(withI18n(<SignInFlow account={{ ...limited, phase: { phase: "signed_out" } }} embedded={false} />));
    await screen.findByText(t("cloud:error.device_limit_expired"));
    expect(screen.getByLabelText(t("cloud:password.email"))).toBeTruthy();
  });
});

describe("应用内登录 · 撞上限", () => {
  it("密码登录撞上设备上限：不换站，只重取账户状态（选设备那一步由账户状态驱动）", async () => {
    setCommandReply("accountSignInBegin", {
      status: "ok",
      data: {
        step: "ready",
        server: { origin: "https://cloud.example", display_name: null, kind: "official" },
        email: "a@b.c",
        methods: null,
        invite: false,
      },
    });
    setCommandReply("accountSignInPassword", { status: "ok", data: { outcome: "device_limit_reached" } });
    const account: AccountStateDto = { server: null, phase: { phase: "signed_out" }, incoming_link: null };
    render(withI18n(<SignInFlow account={account} embedded={false} />));
    fireEvent.change(screen.getByLabelText(t("cloud:password.email")), { target: { value: "a@b.c" } });
    fireEvent.click(screen.getByRole("button", { name: t("cloud:signin.continue") }));
    fireEvent.change(await screen.findByLabelText(t("cloud:password.password")), { target: { value: "pw" } });
    fireEvent.click(screen.getByRole("button", { name: t("cloud:password.submit") }));
    await waitFor(() => expect(commandMock("accountSignInPassword")).toHaveBeenCalledWith("pw"));
    await waitFor(() => expect(commandMock("accountState")).toHaveBeenCalled());
    expect(screen.getByLabelText(t("cloud:password.password"))).toBeTruthy();
  });
});
