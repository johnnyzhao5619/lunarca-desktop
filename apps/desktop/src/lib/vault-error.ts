// Vault 错误串 → 用户读得懂的一句话（纯函数，ADR-105 / TDD-15 §4.3）。
//
// Rust 侧 `vault_err` 发的是**稳定码 + 载荷**（`VAULT_NOT_FOUND: Notes/a.md`），措辞在
// 这里按码取。它此前发的是 `VaultError::to_string()`——措辞、载荷、语言三样黏成一条
// 中文，从一百八十个调用点流进 IPC：英文界面照样读到「笔记不存在：…」，而前端拿不到码
// 可以映射，只能把后端那句原样摆上屏幕。
//
// **载荷照原样带上，不翻译**：它是一条路径、一个 OS errno、或一句已经说清的原因，
// 三者都不随界面语言变；而「是哪一篇」恰恰是这几个码里用户最需要的一格。

import { i18n } from "../i18n/i18n";

// **取 `i18n` 单例而不是收一个 `t` 参数**：这些错误从 hook、事件回调、非组件模块里
// 都会冒出来（`use-settings.ts`、`use-vault-search.ts`…），要 `t` 就等于要求每个调用点
// 先有一个 React 上下文——而那正是「算了，直接把原串摆上去吧」的由来。同一个单例
// `i18n.exists` 本来就在用。

/** `CODE: 载荷` —— 与本仓既有机器前缀同一形状（`DISTILL_INELIGIBLE: …`）。 */
const SHAPE = /^([A-Z][A-Z0-9_]{2,}):\s*([\s\S]*)$/u;

/**
 * 把一条命令错误串翻成人话；**不是 Vault 错误就原样返回**。
 *
 * 返回原串而不是回落到「操作失败」：这条通道上跑的不只有 `VaultError`
 * （`INGEST_INELIGIBLE`、`DISTILL_INELIGIBLE` 等都带着自己已经本地化好的后半句），
 * 一律吃掉会把那些精心写过的话换成一句中性废话。
 */
export function vaultErrorText(raw: string): string {
  const trimmed = raw.trim();
  // **裸码也要认**（ADR-108）：`JobError` 的三处消费面现在只发码（`UPSTREAM`、
  // `EMPTY_AFTER_THINKING`…），不再拼那句中文诊断。没有这一支，它们会以一串
  // SCREAMING 直接上屏——比改之前更糟。
  const bare = /^[A-Z][A-Z0-9_]{2,}$/u.exec(trimmed);
  if (bare !== null) {
    return lookup(trimmed, "") ?? trimmed;
  }
  const match = SHAPE.exec(trimmed);
  if (match === null) {
    return raw;
  }
  const [, code, detail] = match;
  const sentence = lookup(code, detail);
  if (sentence !== null) {
    return sentence;
  }
  // **两张表都没有，就把机器前缀剥掉，只留那半句人话**（真机 2026-08-31）。
  //
  // `coded_error` 产出的形状是 `CODE: 已经本地化好的句子`——后半句本来就是给用户看的，
  // 而前面那个码是**契约**（调用方按它分支、日志按它聚类），对用户毫无意义。真机上它
  // 直接摆在弹窗里：`VAULT_NESTED: 该目录与已有笔记库「LunaVault」相互包含…`。
  //
  // **后半句是空的时候不剥**：那说明 ftl 里缺这条 key（`coded_error_args` 取不到消息
  // 时 `unwrap_or_default()`），此刻把码也吞掉，用户拿到的是一个空字符串——什么都不显示
  // 比显示一个码更糟，而那个码正是排查这条缺 key 的唯一线索。
  return detail.trim() === "" ? raw : detail;
}

/**
 * 一条命令错误串的**机器码**（`VAULT_ALREADY_EXISTS`…）；不是这个形状就返回 `null`。
 *
 * 调用方按码**分支**，不按措辞分支：措辞随语言与文案改版变，码是契约。回收站还原
 * 撞上同名文件时要问的那句「要不要留个副本」，判据只能是 `VAULT_ALREADY_EXISTS`。
 * 与 [vaultErrorText] 共用同一条形状判据（`SHAPE` / 裸码），不另立第二套解析。
 */
export function vaultErrorCode(raw: string): string | null {
  const trimmed = raw.trim();
  if (/^[A-Z][A-Z0-9_]{2,}$/u.test(trimmed)) {
    return trimmed;
  }
  return SHAPE.exec(trimmed)?.[1] ?? null;
}

/**
 * 码 → 句子。**两张表按顺序问**：Vault 存储错误自己一张（它们带载荷），其余落
 * `common:failure.*`——那张表本来就是按 `JobErrorCodeDto` 建的，前端各处早在用它。
 *
 * 两张表都问不到时返回 `null`，由调用方决定回落成什么：这条通道上跑的不只有这两族
 * （`INGEST_INELIGIBLE`、`DISTILL_INELIGIBLE` 等各自带着已经本地化好的后半句），一律吃掉
 * 会把那些精心写过的话换成一句中性废话。
 */
function lookup(code: string, detail: string): string | null {
  for (const key of [`common:vaultError.${code}`, `common:failure.${code}`]) {
    if (i18n.exists(key)) {
      return i18n.t(key, { detail });
    }
  }
  return null;
}

/**
 * 这条失败其实是「用户在文件选择器上按了取消」。
 *
 * 取消是**意思表示，不是错误**：调用方该回到什么都没发生的状态，而不是在屏幕上留下
 * 一行红字（真机 2026-09-11：导入组织配置文件时按取消，卡片上留下「未选择任何内容，
 * 操作已取消」并染成错误色）。判据走既有的码提取，不在调用点比字符串——Rust 侧有六个
 * 产生点，比字符串的写法会被抄成六份。
 */
export function isDialogCancelled(raw: string): boolean {
  return vaultErrorCode(raw) === "DIALOG_CANCELLED";
}
