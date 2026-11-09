// @vitest-environment happy-dom
/**
 * 登录（ADR-155 统一登录，ADR-158 邮箱优先）：像常见应用那样「邮箱 → 将登录到 X → 密码 → 进入」。
 *
 * 钉这些事：
 * 1. 第一站只问邮箱，没有「Lunarca 云 / 我的组织」两个页签；「登录到」默认是 Rust 按邮箱找的那台，
 *    记住的是别的服务器时可一键选回 Lunarca 云，选「其他服务器」才填地址；
 * 2. 密码那一站只交密码（邮箱与服务器上一站已在 Rust 里选定）；登录方式不知道时给密码与一次性码；
 * 3. 忘记密码 / 创建账户 / 手上有码，都是「码 → 设密码」同一条路，码与新密码各只发一次；
 * 4. 文案只说「邮箱」，中英齐备。
 */

import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));
vi.mock("../../ipc/cloud-bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import type { AccountServerDto, AccountStateDto, SignInMethodsDto } from "../../ipc/cloud-bindings";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import cloudEn from "../../i18n/locales/en-US/cloud.json";
import cloudZh from "../../i18n/locales/zh-CN/cloud.json";
import { SignInFlow } from "./SignIn";

const signedOut: AccountStateDto = { server: null, phase: { phase: "signed_out" }, incoming_link: null };
const corp: AccountServerDto = { origin: "https://10.0.0.62:8443", display_name: "测试服务端", kind: "organization" };
const methods: SignInMethodsDto = {
  password: true,
  browser: false,
  providers: [{ id: "00000000-0000-0000-0000-0000000000a1", name: "Okta", kind: "org" }],
  admission: "invite",
  password_reset: true,
};

function ready(
  over: { methods?: SignInMethodsDto | null; invite?: boolean; server?: AccountServerDto } = {},
) {
  return {
    status: "ok",
    data: {
      step: "ready",
      server: over.server ?? corp,
      email: "alice@corp.example",
      methods: over.methods === undefined ? methods : over.methods,
      invite: over.invite ?? false,
    },
  };
}

function mount(account: AccountStateDto = signedOut, onOpenOrgConfig?: () => void) {
  return render(withI18n(createElement(SignInFlow, { account, embedded: false, onOpenOrgConfig })));
}

function enterEmail(value = "Alice@Corp.Example") {
  fireEvent.change(screen.getByLabelText(t("cloud:password.email")), { target: { value } });
  fireEvent.click(screen.getByRole("button", { name: t("cloud:signin.continue") }));
}

async function toPassword(over: Parameters<typeof ready>[0] = {}, onOpenOrgConfig?: () => void) {
  setCommandReply("accountSignInBegin", ready(over));
  mount(signedOut, onOpenOrgConfig);
  enterEmail();
  await screen.findByText(t("cloud:signin.signing_in_to", { name: "测试服务端" }));
}

beforeEach(resetBindingsDouble);
afterEach(cleanup);

describe("登录 · 第一站只问邮箱", () => {
  it("没有页签；填邮箱即按邮箱找服务器", async () => {
    setCommandReply("accountSignInBegin", ready());
    mount();
    expect(screen.queryByRole("radiogroup")).toBeNull();
    expect(screen.queryByLabelText(t("cloud:signin.server_input"))).toBeNull();
    enterEmail();
    await waitFor(() =>
      expect(commandMock("accountSignInBegin")).toHaveBeenCalledWith({ kind: "discover", email: "Alice@Corp.Example" }),
    );
    expect(await screen.findByText("alice@corp.example")).toBeTruthy();
  });

  it("「登录到」选其他服务器才给地址框，交给 Rust 的是邮箱 + 地址", async () => {
    setCommandReply("accountSignInBegin", ready());
    mount();
    fireEvent.change(screen.getByLabelText(t("cloud:signin.server_choice.label")), { target: { value: "other" } });
    fireEvent.change(screen.getByLabelText(t("cloud:signin.server_input")), {
      target: { value: "https://10.0.0.62:8443" },
    });
    enterEmail("a@b.c");
    await waitFor(() =>
      expect(commandMock("accountSignInBegin")).toHaveBeenCalledWith({
        kind: "server",
        email: "a@b.c",
        input: "https://10.0.0.62:8443",
      }),
    );
  });

  it("记住的是别的服务器：默认去它，也能一键选回 Lunarca 云（不必知道 API 地址）", async () => {
    setCommandReply("accountSignInBegin", ready());
    mount({ ...signedOut, server: corp });
    const picker = screen.getByLabelText(t("cloud:signin.server_choice.label"));
    expect(screen.getByRole("option", { name: t("cloud:signin.server_choice.remembered", { name: "测试服务端" }) })).toBeTruthy();
    fireEvent.change(picker, { target: { value: "official" } });
    enterEmail("a@b.c");
    await waitFor(() =>
      expect(commandMock("accountSignInBegin")).toHaveBeenCalledWith({ kind: "official", email: "a@b.c" }),
    );
  });

  it("切换服务器退出后：「登录到」一开始就落在另一侧", () => {
    render(withI18n(createElement(SignInFlow, { account: { ...signedOut, server: corp }, embedded: false, initialChoice: "official" })));
    expect((screen.getByLabelText(t("cloud:signin.server_choice.label")) as HTMLSelectElement).value).toBe("official");
    cleanup();
    // 没有单列的 Lunarca 云一项时落回默认档（它此刻就是 Lunarca 云）。
    render(withI18n(createElement(SignInFlow, { account: signedOut, embedded: false, initialChoice: "official" })));
    expect((screen.getByLabelText(t("cloud:signin.server_choice.label")) as HTMLSelectElement).value).toBe("default");
  });

  it("没记住别的服务器时不重复列出 Lunarca 云", () => {
    mount({ ...signedOut, server: { origin: "https://api.lunarca.app", display_name: null, kind: "official" } });
    expect(screen.getAllByRole("option", { name: t("cloud:signin.official") })).toHaveLength(1);
  });

  it("送达的邀请链接优先：不给「登录到」，继续时带上邮箱", async () => {
    setCommandReply("accountSignInBegin", ready({ invite: true }));
    mount({
      ...signedOut,
      server: corp,
      incoming_link: { server: "https://new.example:8443", display_name: "新组织", has_invite: true },
    });
    expect(screen.getByText(t("cloud:signin.incoming", { name: "新组织" }))).toBeTruthy();
    expect(screen.queryByLabelText(t("cloud:signin.server_choice.label"))).toBeNull();
    enterEmail("a@b.c");
    await waitFor(() =>
      expect(commandMock("accountSignInBegin")).toHaveBeenCalledWith({ kind: "incoming_link", email: "a@b.c" }),
    );
  });

  it("其他服务器里填的是网站：说清那里没有服务端，并指向「登录到 › Lunarca 云」", async () => {
    setCommandReply("accountSignInBegin", {
      status: "error",
      error: { code: "no_server_at_address", host: "lunarca.app" },
    });
    mount();
    fireEvent.change(screen.getByLabelText(t("cloud:signin.server_choice.label")), { target: { value: "other" } });
    fireEvent.change(screen.getByLabelText(t("cloud:signin.server_input")), { target: { value: "lunarca.app" } });
    enterEmail("a@b.c");
    expect(await screen.findByText(t("cloud:error.no_server_at_address", { host: "lunarca.app" }))).toBeTruthy();
    expect(screen.getByText(t("cloud:error.no_server_at_address_action"))).toBeTruthy();
  });

  it("出错就地说原因（邮箱形状由 Rust 判）", async () => {
    setCommandReply("accountSignInBegin", { status: "error", error: { code: "not_an_email" } });
    mount();
    enterEmail("alice");
    expect(await screen.findByText(t("cloud:error.not_an_email"))).toBeTruthy();
  });
});

describe("登录 · 密码那一站", () => {
  it("只交密码；单点登录按 capabilities 给", async () => {
    setCommandReply("accountSignInPassword", { status: "ok", data: { outcome: "signed_in" } });
    await toPassword();
    expect(screen.getByRole("button", { name: t("cloud:signin.sso", { name: "Okta" }) })).toBeTruthy();
    expect(screen.queryByRole("button", { name: t("cloud:password.create_account") })).toBeNull();
    fireEvent.change(screen.getByLabelText(t("cloud:password.password")), { target: { value: "pw-1" } });
    fireEvent.click(screen.getByRole("button", { name: t("cloud:password.submit") }));
    await waitFor(() => expect(commandMock("accountSignInPassword")).toHaveBeenCalledWith("pw-1"));
    await waitFor(() => expect(commandMock("accountState")).toHaveBeenCalled());
  });

  it("Google / Facebook 身份源画成「用 X 继续」，点了带着它的 id 走浏览器登录（ADR-171）", async () => {
    setCommandReply("accountSignInBrowser", { status: "ok", data: null });
    const google = { id: "00000000-0000-0000-0000-0000000000b2", name: "Google", kind: "google" as const };
    const facebook = { id: "00000000-0000-0000-0000-0000000000c3", name: "Facebook", kind: "facebook" as const };
    await toPassword({ methods: { ...methods, admission: "open", providers: [...methods.providers, google, facebook] } });
    fireEvent.click(screen.getByRole("button", { name: t("cloud:signin.google") }));
    await waitFor(() => expect(commandMock("accountSignInBrowser")).toHaveBeenCalledWith(google.id));
    fireEvent.click(screen.getByRole("button", { name: t("cloud:signin.facebook") }));
    await waitFor(() => expect(commandMock("accountSignInBrowser")).toHaveBeenCalledWith(facebook.id));
    fireEvent.click(screen.getByRole("button", { name: t("cloud:signin.sso", { name: "Okta" }) }));
    await waitFor(() =>
      expect(commandMock("accountSignInBrowser")).toHaveBeenCalledWith("00000000-0000-0000-0000-0000000000a1"),
    );
  });

  it("登录方式不知道（自签、还没建立信任）：给密码、一次性码、创建账户，并说清单点登录要配置文件", async () => {
    const openOrgConfig = vi.fn();
    await toPassword({ methods: null }, openOrgConfig);
    expect(screen.getByLabelText(t("cloud:password.password"))).toBeTruthy();
    expect(screen.getByRole("button", { name: t("cloud:password.have_code") })).toBeTruthy();
    expect(screen.getByRole("button", { name: t("cloud:password.create_account") })).toBeTruthy();
    expect(screen.getByText(t("cloud:signin.sso_needs_config"))).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: t("cloud:signin.open_org_config") }));
    expect(openOrgConfig).toHaveBeenCalledTimes(1);
  });

  it("服务器不开放密码：不给密码框", async () => {
    await toPassword({ methods: { ...methods, password: false, browser: true } });
    expect(screen.queryByLabelText(t("cloud:password.password"))).toBeNull();
    expect(screen.getByRole("button", { name: t("cloud:signin.browser") })).toBeTruthy();
  });

  it("身份变了：停下来请用户核对", async () => {
    setCommandReply("accountSignInPassword", {
      status: "ok",
      data: {
        outcome: "needs_trust_confirmation",
        trust: {
          server: "10.0.0.62:8443",
          state: "changed",
          pinned: null,
          offered: null,
          last_error: null,
        },
      },
    });
    await toPassword();
    fireEvent.change(screen.getByLabelText(t("cloud:password.password")), { target: { value: "pw-1" } });
    fireEvent.click(screen.getByRole("button", { name: t("cloud:password.submit") }));
    await waitFor(() => expect(screen.queryByLabelText(t("cloud:password.password"))).toBeNull());
  });

  it("链接带了邀请码：接受邀请 → 设密码", async () => {
    setCommandReply("accountSignInCode", { status: "ok", data: { outcome: "set_password" } });
    await toPassword({ invite: true });
    fireEvent.click(screen.getByRole("button", { name: t("cloud:password.invite_accept") }));
    await waitFor(() => expect(commandMock("accountSignInCode")).toHaveBeenCalledWith(null));
    expect(await screen.findByText(t("cloud:password.set_title"))).toBeTruthy();
  });
});

describe("登录 · 一次性码 → 设密码", () => {
  it("忘记密码：发码 → 输码 → 设密码 → 进入；没收到的帮助在场", async () => {
    setCommandReply("accountSignInCode", { status: "ok", data: { outcome: "set_password" } });
    setCommandReply("accountSignInSetPassword", { status: "ok", data: { outcome: "signed_in" } });
    await toPassword();
    fireEvent.click(screen.getByRole("button", { name: t("cloud:password.forgot") }));
    fireEvent.click(screen.getByRole("button", { name: t("cloud:code.send") }));
    await waitFor(() => expect(commandMock("accountSignInRequestCode")).toHaveBeenCalledWith("reset"));
    expect(await screen.findByText(t("cloud:code.sent", { email: "alice@corp.example" }))).toBeTruthy();
    expect((screen.getByRole("button", { name: /60/ }) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByText(t("cloud:password.not_received"))).toBeTruthy();

    fireEvent.change(screen.getByLabelText(t("cloud:code.code")), { target: { value: "abcd-efgh-23" } });
    fireEvent.click(screen.getByRole("button", { name: t("cloud:code.verify") }));
    await waitFor(() => expect(commandMock("accountSignInCode")).toHaveBeenCalledWith("abcd-efgh-23"));

    await screen.findByText(t("cloud:password.set_title"));
    fireEvent.change(screen.getByLabelText(t("cloud:password.new_password")), { target: { value: "a new passphrase" } });
    fireEvent.change(screen.getByLabelText(t("cloud:password.confirm_password")), {
      target: { value: "a new passphrase!" },
    });
    fireEvent.click(screen.getByRole("button", { name: t("cloud:password.set_submit") }));
    expect(await screen.findByText(t("cloud:password.mismatch"))).toBeTruthy();
    expect(commandMock("accountSignInSetPassword")).not.toHaveBeenCalled();
    fireEvent.change(screen.getByLabelText(t("cloud:password.confirm_password")), {
      target: { value: "a new passphrase" },
    });
    fireEvent.click(screen.getByRole("button", { name: t("cloud:password.set_submit") }));
    await waitFor(() =>
      expect(commandMock("accountSignInSetPassword")).toHaveBeenCalledWith("a new passphrase"),
    );
  });

  /**
   * 官方云上创建账户之前要读到条款与隐私政策（两个入口在系统浏览器里打开）；组织服务端有组织自己的
   * 条款，客户端不替它说——上面那条用例走的就是组织服务端。
   */
  it("官方云创建账户时给出条款与隐私政策，组织服务端不给", async () => {
    const official: AccountServerDto = { origin: "https://api.lunarca.app", display_name: null, kind: "official" };
    setCommandReply("accountSignInBegin", ready({ methods: { ...methods, admission: "open" }, server: official }));
    mount();
    enterEmail();
    await screen.findByText(t("cloud:signin.signing_in_to", { name: t("cloud:signin.official") }));
    fireEvent.click(screen.getByRole("button", { name: t("cloud:password.create_account") }));
    expect(screen.getByText(t("cloud:code.signup_consent"))).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: t("cloud:code.signup_terms") }));
    fireEvent.click(screen.getByRole("button", { name: t("cloud:code.signup_privacy") }));
    expect(commandMock("openExternalUrl").mock.calls).toEqual([
      [t("cloud:code.signup_terms_url")],
      [t("cloud:code.signup_privacy_url")],
    ]);
    // 官网不按浏览器语言跳转：英文界面要直接给英文页（2026-10-28 条款上线）。
    expect([cloudEn.code.signup_terms_url, cloudEn.code.signup_privacy_url]).toEqual([
      "https://lunarca.app/en/terms.html",
      "https://lunarca.app/en/cloud-privacy.html",
    ]);
    cleanup();

    await toPassword({ methods: { ...methods, admission: "open" } });
    fireEvent.click(screen.getByRole("button", { name: t("cloud:password.create_account") }));
    expect(screen.queryByText(t("cloud:code.signup_consent"))).toBeNull();
  });

  it("创建账户发的是注册码；手上有码则直接输", async () => {
    await toPassword({ methods: { ...methods, admission: "open" } });
    fireEvent.click(screen.getByRole("button", { name: t("cloud:password.create_account") }));
    fireEvent.click(screen.getByRole("button", { name: t("cloud:code.send") }));
    await waitFor(() => expect(commandMock("accountSignInRequestCode")).toHaveBeenCalledWith("signup"));
    fireEvent.click(screen.getByRole("button", { name: t("cloud:code.back") }));
    fireEvent.click(screen.getByRole("button", { name: t("cloud:password.have_code") }));
    expect(screen.getByLabelText(t("cloud:code.code"))).toBeTruthy();
    expect(commandMock("accountSignInRequestCode")).toHaveBeenCalledTimes(1);
  });

  it("一次性邮箱域名被拒：说「请使用常用邮箱注册」", async () => {
    setCommandReply("accountSignInRequestCode", { status: "error", error: { code: "email_domain_not_allowed" } });
    await toPassword({ methods: { ...methods, admission: "open" } });
    fireEvent.click(screen.getByRole("button", { name: t("cloud:password.create_account") }));
    fireEvent.click(screen.getByRole("button", { name: t("cloud:code.send") }));
    expect(await screen.findByText(t("cloud:error.email_domain_not_allowed"))).toBeTruthy();
  });
});

describe("登录 · 文案", () => {
  it("中英齐备；中文只说「邮箱」", () => {
    for (const locale of [cloudZh, cloudEn]) {
      for (const key of ["server_input", "sso_needs_config", "change"] as const) {
        expect(locale.signin[key].length).toBeGreaterThan(0);
      }
      for (const key of ["label", "remembered", "other"] as const) {
        expect(locale.signin.server_choice[key].length).toBeGreaterThan(0);
      }
      for (const key of ["have_code", "create_account", "set_title", "invite_accept"] as const) {
        expect(locale.password[key].length).toBeGreaterThan(0);
      }
      expect(locale.code.sent).toContain("{{email}}");
      expect(locale.error.server_identity_rejected.length).toBeGreaterThan(0);
    }
    expect(JSON.stringify(cloudZh)).not.toContain("工作邮箱");
    expect(JSON.stringify(cloudEn).toLowerCase()).not.toContain("work email");
  });
});
