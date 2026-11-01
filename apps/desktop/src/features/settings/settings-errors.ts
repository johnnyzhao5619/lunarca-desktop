/**
 * 设置写入失败的**稳定 code → i18n key** 映射（前端不解析人话）。
 *
 * Rust 侧 `set_setting` 以 `<CODE>: <细节>` 形式返回（`luna_config::Rejection::code()` +
 * 快捷键冲突），本模块把 code 翻成用户能读懂的一句话；未知错误如实原样展示，
 * 绝不吞掉（宁可显示英文细节，也不显示一句"操作失败"）。
 */

import type { SettingValueDto } from "../../ipc/bindings";
import { formatShortcut } from "../../lib/shortcut-display";

/** 一次失败的呈现材料：i18n key + 插值参数（`raw` = 未识别时的原始错误串）。 */
export interface SettingErrorView {
  i18nKey: string;
  params?: Record<string, string>;
  raw?: string;
}

const CODE_TO_KEY: Record<string, string> = {
  SETTING_TYPE: "error.type",
  SETTING_OPTION: "error.option",
  SETTING_RANGE: "error.range",
  // 有序多选的两条新失败模式（TDD-05 §7.5a）。少了它们，用户删到只剩写作入口时
  // 看到的是 `error.unknown` 里那句英文原文——而"每类至少一项"这条规则的判定归 Rust，
  // 前端**只负责把理由说成人话**，说不出来等于这条规则从未存在过。
  SETTING_DUPLICATE_OPTION: "error.duplicate_option",
  SETTING_REQUIRED_CATEGORY: "error.required_category",
  SETTING_DECIMAL: "error.decimal",
  SETTING_PATH: "error.path",
  SETTING_FILENAME_TEMPLATE: "error.filename_template",
  SETTING_TEXT_TOO_LONG: "error.text_too_long",
  SETTING_SHORTCUT: "error.shortcut",
  SETTING_SHORTCUT_RESERVED: "error.shortcut_reserved",
  SETTING_SHORTCUT_KEY: "error.shortcut_key",
  SETTING_SHORTCUT_EDITOR: "error.shortcut_editor",
  SETTING_SHORTCUT_CONFLICT: "error.shortcut_conflict",
  // 设置文件落盘失败（磁盘满 / 无写权限）：不是值不合法，值本身没问题。
  SETTING_PERSIST: "error.persist",
  SETTING_RESET_STALE: "error.reset_stale",
};

/** 解析 Rust 错误串为可渲染的失败视图。 */
export function parseSettingError(message: string): SettingErrorView {
  const separator = message.indexOf(":");
  const code = separator === -1 ? message.trim() : message.slice(0, separator).trim();
  const detail = separator === -1 ? "" : message.slice(separator + 1).trim();
  const i18nKey = CODE_TO_KEY[code];
  if (!i18nKey) {
    return { i18nKey: "error.unknown", params: { detail: message }, raw: message };
  }
  // 冲突错误的细节是**另一个设置 key**，交由调用方翻成该项标签（而不是把 key 甩给用户）。
  return { i18nKey, params: { detail } };
}

/** 一次写入失败：错误串 + 当时试的那个值（行上显示的仍是旧值，故必须带上）。 */
export interface SettingWriteFailure {
  message: string;
  attempted: SettingValueDto;
}

/**
 * 一次写入失败 → 用户读得懂的那句话。
 *
 * 抽成函数是因为消费方不止设置页：引导的快捷键速改也要显示同一句判语，而这段有三处
 * 非平凡处理——冲突错误的 `detail` 是**另一个设置 key**（要翻成那一项的标签，不能把
 * `shortcuts.popup_translate` 甩给用户看）、`attempted` 要按展示规约格式化成键帽、
 * 未识别的 code 如实原样展示。各写一份的结果就是其中一处漏掉翻译，屏幕上出现
 * `SETTING_SHORTCUT_CONFLICT: shortcuts.popup_translate`。
 *
 * @param t `settings` 命名空间的翻译函数。
 */
export function settingErrorText(
  failure: SettingWriteFailure,
  t: (key: string, options?: Record<string, unknown>) => string,
  /**
   * 该项自己的契约参数（当前只有数值范围）。
   *
   * 「超出允许范围」不说范围是多少，等于把用户留在原地——而 `min`/`max` 注册表本来就
   * 随规格下发了，这里只是把它渲染出来，不是在前端再判一次合法性（ADR-003）。
   */
  extra?: Record<string, unknown>,
): string {
  const view = parseSettingError(failure.message);
  const rawDetail = view.params?.detail ?? "";
  const detail =
    view.i18nKey === "error.shortcut_conflict" && rawDetail
      ? t(rawDetail.replace(/^settings:/, ""), { defaultValue: rawDetail })
      : rawDetail;
  const attempted =
    failure.attempted.kind === "str" ? formatShortcut(failure.attempted.value) : "";
  return t(view.i18nKey, { ...view.params, ...extra, detail, attempted });
}

/** 快捷键运行诊断 code → i18n key（PRD-09-003 ②③；code 清单见 Rust `ShortcutDiagnosticDto`）。 */
export function shortcutDiagnosticKey(code: string): string {
  switch (code) {
    case "unavailable":
    case "invalid":
    case "reserved_system":
    case "accessibility_required":
    case "tap_failed":
    case "double_tap_unsupported":
      return `shortcuts.diagnostic_${code}`;
    default:
      return "shortcuts.diagnostic_unknown";
  }
}

/** 该诊断是否能由用户就地修复——授予辅助功能权限。 */
export function shortcutDiagnosticNeedsAccessibility(code: string): boolean {
  return code === "accessibility_required";
}
