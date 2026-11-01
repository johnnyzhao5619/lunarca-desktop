/**
 * **每一个 `VaultError` 码都要有中英文案；没有对应变体的文案不许存在**（ADR-105）。
 *
 * 判据的全集**从 Rust 源码取**，不在这里手抄：抄一份的话，后端新增变体、这里没跟上，
 * 用户看到的是一条原样回显的诊断串（`VAULT_XXX: /Users/…`），而没有任何东西会红——
 * 那正是这一批要消灭的形态，抄一份等于把它换个地方重演一遍。
 */

import { describe, expect, it } from "vitest";

import zhCommon from "../i18n/locales/zh-CN/common.json";
import enCommon from "../i18n/locales/en-US/common.json";
import { vaultErrorText, isDialogCancelled } from "./vault-error";

const app = import.meta.glob("../**/*.{ts,tsx}", {
  eager: true,
  query: "?raw",
  import: "default",
}) as Record<string, string>;

const errorRs = import.meta.glob("../../../../crates/luna-vault/src/error.rs", {
  eager: true,
  query: "?raw",
  import: "default",
}) as Record<string, string>;

/** `VaultError::code()` 那张 match 表里的全部码。 */
function codesFromRust(): string[] {
  const source = Object.values(errorRs)[0] ?? "";
  const table = source.slice(source.indexOf("pub fn code("), source.indexOf("pub type Result"));
  const found = [...table.matchAll(/=>\s*"([A-Z][A-Z0-9_]{2,})"/g)].map((m) => m[1]);
  return [...new Set(found)].sort();
}

const CODES = codesFromRust();

describe("Vault 错误码 ↔ 中英文案（ADR-105）", () => {
  it("从 Rust 取到了全集（判据本身失灵时这条先红）", () => {
    expect(CODES.length).toBeGreaterThanOrEqual(14);
    expect(CODES).toContain("VAULT_NOT_FOUND");
    expect(CODES).toContain("LIST_CURSOR_INVALID");
  });

  it("每个码都有中英文案，且都带上载荷", () => {
    for (const [name, bundle] of [
      ["zh", zhCommon],
      ["en", enCommon],
    ] as const) {
      const table = (bundle as { vaultError: Record<string, string> }).vaultError;
      for (const code of CODES) {
        const text = table[code];
        expect(text, `${name} 缺 common:vaultError.${code}`).toBeTruthy();
        // 载荷必须在句子里：「找不到这篇笔记」而不说是哪一篇，等于把用户手上唯一
        // 能行动的信息扣下了。
        expect(text, `${name} 的 ${code} 没有带上 {{detail}}`).toContain("{{detail}}");
      }
      // 反向：文案表里不许有 Rust 侧不存在的码（拼错的那一条永远不会被任何东西碰到）。
      for (const key of Object.keys(table)) {
        expect(CODES, `${name} 的 vaultError.${key} 在 Rust 侧没有对应变体`).toContain(key);
      }
    }
  });

  /**
   * **把命令错误摆上屏幕的地方，一处都不许绕过渲染口**（ADR-105）。
   *
   * 这是这一批的成败所在：Rust 那一半改成发「码 + 载荷」之后，任何一个仍然直接
   * `setError(result.error)` 的地方，用户看到的不再是一句中文，而是一条**原样的诊断串**
   * （`VAULT_NOT_FOUND: Notes/…`）——比改之前更糟。判据因此落在源码形状上，而不是
   * 靠改的时候记得。
   *
   * 判据只认「把错误交给一个状态设置器」这一种形状；按内容分支（`result.error.startsWith`）
   * 与传给日志的**不在射程内**，它们要的正是原始码。
   *
   * **判据锚在形状上（`set…(x.error)`），不锚在设置器的名字上**（真机 2026-08-31）。
   * 第一版枚举了四个名字（`setError|setStatus|setNotice|setFeedback`），于是
   * `setRenameError(result.error)` 从缝里漏了过去——真机上重命名撞重名，界面原样摆出
   * `VAULT_ALREADY_EXISTS: /Users/…/笔记.md`，中英文都一样。**枚举名字的判据只能挡住
   * 你当时想得起来的那几个名字**，而下一个人不会来读这条正则。同一批里另有 11 处同样
   * 漏网，全部由改宽后的判据一次扫出。
   */
  it("没有任何渲染点直接把 `result.error` 摆上屏幕", () => {
    const setters = /\bset[A-Za-z]*\(\s*\w+\.error\s*\)/g;
    const offenders: string[] = [];
    for (const [path, text] of Object.entries(app)) {
      if (path.includes("vault-error")) continue;
      for (const hit of text.match(setters) ?? []) {
        // 这几处的 `.error` 是结构化的 `CloudError`（自带 code 与专属渲染），不是这条
        // 通道上的字符串——它们由 typecheck 拦着（把 `vaultErrorText` 套上去当场类型不符），
        // 不在本判据射程内。豁免逐个列名并写明理由，不给通配。
        // `SyncSection` / `SyncSetupDialog` / `SyncRestoreDialog` / `CloudVaultList` 存的是同步命令的**稳定码**（`sync_not_entitled` 之类，不带载荷），
        // 渲染时统一过 `syncErrorText`（`cloud:sync.error.<code>`，认不出给通用句）——
        // 同步码不走 Vault 那条「码: 诊断串」通道，`vaultErrorText` 对它是错的渲染口。
        if (
          path.includes("ModelCatalogSection") ||
          path.includes("AccountPanel") ||
          path.includes("account/SignIn") ||
          // 登录撞上设备上限那一步（纵切 3 C5）：存的同样是结构化 `CloudError`，经 `errorText` 渲染。
          path.includes("account/DeviceLimitPicker") ||
          // 登录状态下改密码（ADR-158 ⑤）：同上，结构化 `CloudError` 经 `errorText` 渲染。
          path.includes("account/ChangePassword") ||
          path.includes("account/ServerIdentity") ||
          path.includes("account/SyncSection") ||
          path.includes("account/SyncSetupDialog") ||
          path.includes("account/SyncRestoreDialog") ||
          path.includes("account/CloudVaultList")
        ) {
          continue;
        }
        offenders.push(`${path}: ${hit}`);
      }
    }
    expect(
      offenders,
      "这些地方会把诊断串原样摆给用户，请改用 `vaultErrorText(...)`",
    ).toEqual([]);
  });

  it("认不出的码：剥掉机器前缀，只留那半句人话", () => {
    // 这条通道上跑的不只有 VaultError——`coded_error` 那一族各自带着**已经本地化好的**
    // 说明，前面那个码是契约（调用方分支、日志聚类），对用户毫无意义。真机 2026-08-31：
    // 它直接摆在弹窗里，`VAULT_NESTED: 该目录与已有笔记库「LunaVault」相互包含…`。
    expect(vaultErrorText("DISTILL_INELIGIBLE: 这一篇不能整理成知识笔记")).toBe(
      "这一篇不能整理成知识笔记",
    );
    expect(vaultErrorText("VAULT_NESTED: 该目录与已有笔记库「X」相互包含。")).toBe(
      "该目录与已有笔记库「X」相互包含。",
    );
    // **后半句是空的时候不剥**：那说明 ftl 缺这条 key，此刻把码也吞掉，用户拿到的是
    // 一个空串——什么都不显示比显示一个码更糟，而那个码是排查缺 key 的唯一线索。
    expect(vaultErrorText("SOME_MISSING_KEY: ")).toBe("SOME_MISSING_KEY: ");
    // 形状不对的（没有机器前缀）原样返回。
    expect(vaultErrorText("Vault 未打开")).toBe("Vault 未打开");
    expect(vaultErrorText("")).toBe("");
  });

  /**
   * **按前缀分支的消费者不许走渲染口**。
   *
   * `WorkspaceApp` 用 `result.error.startsWith("VOICE_SESSION_BUSY")` 判「是被拒了还是
   * 失败了」。渲染口会把那个前缀剥掉——套上去的话这条分支恒为 false，而**什么都不会红**。
   * 这也正是不在 `commands` 边界统一拦截的理由（ADR-108 ③）：那样做的话这种分支只是
   * 「碰巧还对」，而现在它连碰巧都不成立。
   */
  it("按前缀分支的地方拿的是原始串", () => {
    const view = app["../app/WorkspaceApp.tsx"] ?? "";
    expect(view.length, "WorkspaceApp.tsx 没被 glob 到").toBeGreaterThan(0);
    const line = view
      .split("\n")
      .find((l) => l.includes('startsWith("VOICE_SESSION_BUSY")'));
    expect(line, "那条分支不见了——它没了的话本判据也该跟着删").toBeDefined();
    expect(
      line?.includes("vaultErrorText"),
      "分支读的是渲染口的输出——前缀已被剥掉，这条判断恒为 false",
    ).toBe(false);
    // 渲染口本身照常剥前缀：两件事各归各位，不是「放行某个码」。
    expect(vaultErrorText("VOICE_SESSION_BUSY: 上一场还没释放")).toBe("上一场还没释放");
  });

  /**
   * **裸码必须渲染得出人话**（ADR-108）。
   *
   * 这是「三处消费面改成只发码」的前提：`embed` / `checkup` / `ingest` 的失败此前发的是
   * `format!("{}: {err}", err.code())`——那句 `{err}` 是 `JobError` 的 `Display`，受众是
   * 开发者。改成只发码之后，若前端认不出裸码，用户看到的就是一串 SCREAMING，**比改之前
   * 更糟**。故这一支与那三处是同一笔改动的两半，缺一不可。
   */
  it("裸码走 common:failure.*，认不出的原样返回", async () => {
    const { i18n } = await import("../i18n/i18n");
    await i18n.changeLanguage("zh-CN");
    const upstream = vaultErrorText("UPSTREAM");
    expect(upstream).not.toBe("UPSTREAM");
    expect(upstream.length).toBeGreaterThan(4);
    // ADR-102 那个码同样要认得（它正是从这三条路上冒出来的）。
    expect(vaultErrorText("EMPTY_AFTER_THINKING")).not.toBe("EMPTY_AFTER_THINKING");
    // 认不出的**原样返回**，不吞成一句中性废话：这条通道上跑的不只有 JobError。
    expect(vaultErrorText("SOME_UNKNOWN_CODE")).toBe("SOME_UNKNOWN_CODE");
  });

  it("Vault 错误按码取文案，载荷原样带进去，且随界面语言变", async () => {
    const { i18n } = await import("../i18n/i18n");
    await i18n.changeLanguage("zh-CN");
    const zh = vaultErrorText("VAULT_NOT_FOUND: Notes/a.md");
    expect(zh).toContain("Notes/a.md");
    // 码本身绝不上界面：它是诊断契约，不是给用户看的（TDD-15 §4.3）。
    expect(zh).not.toContain("VAULT_NOT_FOUND");
    await i18n.changeLanguage("en-US");
    const en = vaultErrorText("VAULT_NOT_FOUND: Notes/a.md");
    expect(en).toContain("Notes/a.md");
    expect(en).not.toBe(zh);
  });
});

describe("isDialogCancelled", () => {
  /**
   * 取消是意思表示，不是错误——调用方该回到什么都没发生的状态。
   * 判据走码提取而不是比字符串：Rust 侧有六个产生点，比字符串会被抄成六份，
   * 而抄错一处的表现是屏幕上留下一行红字说「你取消了」。
   */
  it("认得出取消，且不把别的失败当成取消", () => {
    expect(isDialogCancelled("DIALOG_CANCELLED: 未选择任何内容，操作已取消。")).toBe(true);
    expect(isDialogCancelled("DIALOG_CANCELLED: Nothing was selected.")).toBe(true);
    expect(isDialogCancelled("TRUST_PROFILE_INVALID: 这份配置文件不是合法的 JSON")).toBe(false);
    expect(isDialogCancelled("VAULT_NOT_FOUND: Notes/a.md")).toBe(false);
    // 没有码的原始串不算——把它判成取消会把真实故障吞掉。
    expect(isDialogCancelled("用户取消了")).toBe(false);
    expect(isDialogCancelled("")).toBe(false);
  });
});
