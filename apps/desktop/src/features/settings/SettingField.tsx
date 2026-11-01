import { useContext, useState, type MouseEvent as ReactMouseEvent } from "react";
import { useTranslation } from "react-i18next";
import { commands, type SettingSpecDto } from "../../ipc/bindings";
import { ArrowClockwiseIcon, CheckIcon, ClockIcon, CopyIcon } from "../../components/ui/Icon";
import { HelpDetails } from "../../components/ui/HelpDetails";
import { useContextMenu } from "../../components/ContextMenuHost";
import { buildMenu, separator } from "../../lib/context-menu";
import { SettingControl, type SetValue } from "./SettingControls";
import { isModified } from "./settings-filter";
import { settingErrorText, shortcutDiagnosticKey, shortcutDiagnosticNeedsAccessibility } from "./settings-errors";
import { SettingsFieldContext } from "./settings-field-context";
import type { SettingsController } from "./use-settings-controller";
import { goTo } from "../../lib/navigate";

/**
 * 说明在这一行此刻的呈现（D1，2026-10-08）：由 Rust 逐键声明的 `desc_presentation` 决定默认，
 * 两种场合例外——**搜索结果**里一律常显（用户正是凭说明里的词找到这一行的，结果得让他看到
 * 为什么命中），以及调用方**显式传入**的说明（面板按运行时状态给的那一句，不是注册表文案）。
 * 穷尽 `switch`：注册表多一档，这里编译不过。
 */
function descriptionMode(
  presentation: SettingSpecDto["desc_presentation"],
  { searching, explicit }: { searching: boolean; explicit: boolean },
): "inline" | "disclosure" | "hidden" {
  if (searching || explicit) {
    return "inline";
  }
  switch (presentation) {
    case "inline":
      return "inline";
    case "disclosure":
      return "disclosure";
    case "label_only":
      return "hidden";
  }
}

export function SettingField({
  spec,
  controller,
  showPage,
  setValue,
  desc,
  disabled,
  panel = false,
}: {
  spec: SettingSpecDto;
  controller?: SettingsController;
  setValue?: SetValue;
  desc?: string;
  disabled?: boolean;
  panel?: boolean;
  /** 搜索结果里标注该项属于哪一页（否则命中一堆同名开关分不清出处）。 */
  showPage?: boolean;
}) {
  const { t } = useTranslation("settings");
  const contextMenu = useContextMenu();
  const inheritedController = useContext(SettingsFieldContext);
  const feedback = controller ?? inheritedController;
  const error = feedback?.errors[spec.key];
  const pending = feedback?.pendingKeys[spec.key] ?? false;
  const diagnostic = feedback?.diagnostics[spec.key];
  const modified = isModified(spec);
  const description = desc ?? t(spec.desc_i18n_key, { defaultValue: "" });
  const mode = descriptionMode(spec.desc_presentation, {
    searching: Boolean(showPage),
    explicit: desc !== undefined,
  });
  /** 按需展开的说明此刻是否展开（纯视图态，不写任何设置）。 */
  const [helpOpen, setHelpOpen] = useState(false);

  // 控件范围随契约下发，用于给出可操作的错误提示。
  const range =
    (spec.control.kind === "number" || spec.control.kind === "decimal") &&
    spec.control.min !== null &&
    spec.control.max !== null
      ? { min: spec.control.min, max: spec.control.max }
      : undefined;
  const errorText = error ? settingErrorText(error, t, range) : null;

  /**
   * 设置行右键（PRD-07-059）：恢复默认 + 把 key/值抄走。
   */
  function openFieldMenu(event: ReactMouseEvent) {
    contextMenu.open(
      event,
      buildMenu([
        {
          kind: "action",
          id: "reset",
          label: t("action.reset"),
          icon: <ArrowClockwiseIcon size={13} />,
          disabled: !modified || !feedback || !spec.resettable || pending,
          onSelect: () => {
            void feedback?.reset(spec.key);
          },
        },
        separator("copy"),
        {
          kind: "action",
          id: "copyKey",
          label: t("action.copy_key"),
          icon: <CopyIcon size={13} />,
          onSelect: () => {
            void commands.popupCopy(spec.key);
          },
        },
        {
          kind: "action",
          id: "copyValue",
          label: t("action.copy_value"),
          icon: <CopyIcon size={13} />,
          onSelect: () => {
            void commands.popupCopy(String(spec.current_value.value));
          },
        },
      ]),
    );
  }

  // 有序列表需要整行宽度，行尾动作仍保留在首行。
  const wide =
    spec.control.kind === "ordered_multi_select" || spec.control.kind === "ordered_toggle_list";
  const Root = panel ? "fieldset" : "li";
  return (
    <Root
      className={panel ? "luna-settings-field luna-settings-field--group" : "luna-settings-field"}
      disabled={panel ? disabled : undefined}
      data-disabled={disabled}
      data-setting-key={spec.key}
      data-wide={wide || undefined}
      tabIndex={-1}
      onContextMenu={openFieldMenu}
      // 深链定位（`settings://<页>/<key>`）是对这一行本身的程序化聚焦：落到一项按需说明的
      // 设置上时把说明一并展开——用户是被带来看这一项的。键盘 Tab 不会聚焦行根（tabIndex -1）。
      onFocus={(event) => {
        if (mode === "disclosure" && event.target === event.currentTarget) {
          setHelpOpen(true);
        }
      }}
    >
      <div className="luna-settings-field__label">
        <span className="luna-settings-field__title">
          {t(spec.i18n_key)}
          {showPage ? (
            <span className="luna-settings-field__chip">{t(`group.${spec.group}`)}</span>
          ) : null}
          {spec.scope === "vault" ? (
            <span className="luna-settings-field__chip" title={t("scope.vault_hint")}>
              {t("scope.vault")}
            </span>
          ) : null}
          {spec.requires_restart ? (
            <span className="luna-settings-field__restart">{t("action.restart_required")}</span>
          ) : null}
          {spec.control.kind === "shortcut" ? (
            <span className="luna-settings-field__chip">
              {t(spec.control.scope === "global" ? "shortcuts.scope_global" : "shortcuts.scope_in_app")}
            </span>
          ) : null}
          {mode === "disclosure" && description ? (
            <HelpDetails summary={t("action.about_setting", { name: t(spec.i18n_key) })}
              open={helpOpen} onOpenChange={setHelpOpen}><p>{description}</p></HelpDetails>
          ) : null}
        </span>
        {description && mode === "inline" ? (
          <p className="luna-settings-field__desc">
            {description}
          </p>
        ) : null}
        {diagnostic ? (
          <p className="luna-settings-field__warning">
            {t(shortcutDiagnosticKey(diagnostic))}
            {shortcutDiagnosticNeedsAccessibility(diagnostic) ? (
              <button
                type="button"
                className="luna-btn luna-btn--secondary luna-settings-field__warning-action"
                onClick={() => {
                  void commands.requestPermission("accessibility");
                }}
              >
                {t("shortcuts.grant_accessibility")}
              </button>
            ) : null}
          </p>
        ) : null}
        {/* 早已存在的撞键（W3b）：改键时的冲突当场被拒，这里说的是升级加了新默认键、或配置
            是手改的情形。判定在 Rust（与改键同一判据），前端只把那一项的 key 翻成名字。 */}
        {spec.shortcut_conflict ? (
          <p className="luna-settings-field__warning">
            {t("shortcuts.existing_conflict", {
              other: t(spec.shortcut_conflict, { defaultValue: spec.shortcut_conflict }),
            })}
            <button type="button" className="luna-btn luna-btn--secondary luna-btn--sm"
              onClick={() => goTo(`settings://shortcuts/${spec.shortcut_conflict}`)}>
              {t("shortcuts.view_conflict")}
            </button>
          </p>
        ) : null}
        {errorText ? (
          <p className="luna-field-error" role="alert">
            {errorText}
          </p>
        ) : null}
      </div>
      <fieldset className="luna-settings-field__control" disabled={pending || disabled} aria-busy={pending}>
        <SettingControl
          key={feedback?.resetEpochs[spec.key] ?? 0}
          spec={spec}
          advancedMode={feedback?.advancedMode}
          setValue={setValue ?? feedback!.setValue}
          invalid={Boolean(error)}
          clearError={() => feedback?.clearError(spec.key)}
          onVaultChanged={() => {
            void feedback?.refresh();
          }}
        />
      </fieldset>
      <div className="luna-settings-field__action">
        {pending ? <span className="luna-settings-field__saved" data-state="pending" role="status" title={t("action.saving")}>
          <ClockIcon size={16} /><span className="luna-visually-hidden">{t("action.saving")}</span>
        </span> : feedback?.savedKeys[spec.key] ? (
          <span className="luna-settings-field__saved" role="status" title={t("action.saved")}>
            <CheckIcon size={16} />
          </span>
        ) : modified && feedback && spec.resettable ? (
          <button
            type="button"
            className="luna-settings-field__reset"
            title={t("action.reset_hint")}
            aria-label={t("action.reset")}
            disabled={pending || disabled || !spec.resettable}
            onClick={() => {
              void feedback?.reset(spec.key);
            }}
          >
            <ArrowClockwiseIcon size={16} />
          </button>
        ) : null}
      </div>
    </Root>
  );
}
