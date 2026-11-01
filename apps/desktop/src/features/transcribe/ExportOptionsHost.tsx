/**
 * 「导出选项」对话框宿主（PRD-11-004/007）：应用级唯一一张，任何导出入口经
 * `requestExportOptions()` 打开。字段：整体时移、每行字数（自动 / 自定义）、每条行数、
 * 说话人前缀、UTF-8 BOM（默认来自设置 `export.utf8_bom`，改动写回设置）。
 *
 * 只改会话级选项，不发导出——导出仍由各入口自己的按钮发起，对话框关掉后选项继续生效。
 */

import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useDialogBehavior } from "../../hooks/use-dialog-behavior";
import { IconButton } from "../../components/ui/IconButton";
import { CloseIcon } from "../../components/ui/Icon";
import { EXPORT_OPTIONS_EVENT } from "../../lib/app-events";
import { useSettings } from "../settings/use-settings";
import { HAN_VARIANTS, updateExportOptions, useExportOptions } from "./export-options";

const BOM_SETTING = "export.utf8_bom";

export function ExportOptionsHost() {
  const { t } = useTranslation("transcribe");
  const [open, setOpen] = useState(false);
  const options = useExportOptions();
  const { specs, setValue } = useSettings();
  const firstFieldRef = useRef<HTMLInputElement>(null);
  const dialogRef = useDialogBehavior<HTMLDivElement>({
    onClose: () => setOpen(false),
    enabled: open,
    initialFocusRef: firstFieldRef,
  });

  useEffect(() => {
    const onRequest = () => setOpen(true);
    window.addEventListener(EXPORT_OPTIONS_EVENT, onRequest);
    return () => window.removeEventListener(EXPORT_OPTIONS_EVENT, onRequest);
  }, []);

  // BOM 默认值跟设置走：设置读回后同步进会话选项（用户在对话框里改过则以设置为准的那次写回）。
  const bomSpec = specs.find((spec) => spec.key === BOM_SETTING);
  const bomFromSettings = bomSpec?.current_value.kind === "bool" ? bomSpec.current_value.value : null;
  useEffect(() => {
    if (bomFromSettings !== null) {
      updateExportOptions({ utf8_bom: bomFromSettings });
    }
  }, [bomFromSettings]);

  if (!open) {
    return null;
  }

  const lineLenMode = options.max_line_len === null ? "auto" : "custom";

  return (
    <div className="luna-overlay" role="presentation">
      <div
        ref={dialogRef}
        className="luna-dialog luna-dialog--panel luna-export-options"
        role="dialog"
        aria-modal="true"
        aria-labelledby="export-options-title"
      >
        <header className="luna-export-options__header">
          <h2 id="export-options-title" className="luna-export-options__title">
            {t("exportOptions.title")}
          </h2>
          <IconButton
            label={t("exportOptions.close")}
            icon={<CloseIcon size={14} />}
            onClick={() => setOpen(false)}
          />
        </header>
        <p className="luna-export-options__hint">{t("exportOptions.hint")}</p>

        <div className="luna-export-options__body">
          <label className="luna-export-options__field">
            <span className="luna-export-options__label">{t("exportOptions.timeOffset")}</span>
            <span className="luna-export-options__control">
              <input
                ref={firstFieldRef}
                type="number"
                step={100}
                value={options.time_offset_ms}
                onChange={(event) => {
                  const next = Number.parseInt(event.target.value, 10);
                  updateExportOptions({ time_offset_ms: Number.isFinite(next) ? next : 0 });
                }}
              />
              <span className="luna-export-options__unit">{t("exportOptions.ms")}</span>
            </span>
            <span className="luna-export-options__desc">{t("exportOptions.timeOffsetDesc")}</span>
          </label>

          <div className="luna-export-options__field" role="group" aria-label={t("exportOptions.lineLength")}>
            <span className="luna-export-options__label">{t("exportOptions.lineLength")}</span>
            <span className="luna-export-options__control">
              <div className="luna-segmented-control" role="radiogroup" aria-label={t("exportOptions.lineLength")}>
                {(["auto", "custom"] as const).map((mode) => (
                  <button
                    key={mode}
                    type="button"
                    role="radio"
                    aria-checked={lineLenMode === mode}
                    className="luna-segmented-control__item"
                    data-active={lineLenMode === mode}
                    onClick={() =>
                      updateExportOptions({ max_line_len: mode === "auto" ? null : 42 })
                    }
                  >
                    {t(`exportOptions.lineLength_${mode}`)}
                  </button>
                ))}
              </div>
              {lineLenMode === "custom" ? (
                <input
                  type="number"
                  min={8}
                  max={120}
                  value={options.max_line_len ?? 42}
                  aria-label={t("exportOptions.lineLengthValue")}
                  onChange={(event) => {
                    const next = Number.parseInt(event.target.value, 10);
                    updateExportOptions({
                      max_line_len: Number.isFinite(next) ? Math.min(120, Math.max(8, next)) : 42,
                    });
                  }}
                />
              ) : null}
            </span>
            <span className="luna-export-options__desc">{t("exportOptions.lineLengthDesc")}</span>
          </div>

          <div className="luna-export-options__field" role="group" aria-label={t("exportOptions.lines")}>
            <span className="luna-export-options__label">{t("exportOptions.lines")}</span>
            <span className="luna-export-options__control">
              <div className="luna-segmented-control" role="radiogroup" aria-label={t("exportOptions.lines")}>
                {([null, 1, 2, 3] as const).map((value) => (
                  <button
                    key={String(value)}
                    type="button"
                    role="radio"
                    aria-checked={options.max_lines === value}
                    className="luna-segmented-control__item"
                    data-active={options.max_lines === value}
                    onClick={() => updateExportOptions({ max_lines: value })}
                  >
                    {value === null ? t("exportOptions.lines_auto") : String(value)}
                  </button>
                ))}
              </div>
            </span>
          </div>

          <div className="luna-export-options__field" role="group" aria-label={t("exportOptions.han")}>
            <span className="luna-export-options__label">{t("exportOptions.han")}</span>
            <span className="luna-export-options__control">
              <div className="luna-segmented-control" role="radiogroup" aria-label={t("exportOptions.han")}>
                {HAN_VARIANTS.map((value) => (
                  <button
                    key={value}
                    type="button"
                    role="radio"
                    aria-checked={options.han_variant === value}
                    className="luna-segmented-control__item"
                    data-active={options.han_variant === value}
                    onClick={() => updateExportOptions({ han_variant: value })}
                  >
                    {t(`exportOptions.han_${value}`)}
                  </button>
                ))}
              </div>
            </span>
            <span className="luna-export-options__desc">{t("exportOptions.hanDesc")}</span>
          </div>

          <label className="luna-export-options__toggle">
            <input
              type="checkbox"
              checked={options.speaker_prefix}
              onChange={(event) => updateExportOptions({ speaker_prefix: event.target.checked })}
            />
            <span>
              <strong>{t("exportOptions.speaker")}</strong>
              <small>{t("exportOptions.speakerDesc")}</small>
            </span>
          </label>

          <label className="luna-export-options__toggle">
            <input
              type="checkbox"
              checked={options.utf8_bom}
              onChange={(event) => {
                const value = event.target.checked;
                updateExportOptions({ utf8_bom: value });
                void setValue(BOM_SETTING, { kind: "bool", value });
              }}
            />
            <span>
              <strong>{t("exportOptions.bom")}</strong>
              <small>{t("exportOptions.bomDesc")}</small>
            </span>
          </label>
        </div>

        <footer className="luna-export-options__footer">
          <button
            type="button"
            className="luna-btn luna-btn--secondary"
            onClick={() =>
              updateExportOptions({
                time_offset_ms: 0,
                max_line_len: null,
                max_lines: null,
                speaker_prefix: true,
                han_variant: "keep",
              })
            }
          >
            {t("exportOptions.reset")}
          </button>
          <button type="button" className="luna-btn luna-btn--primary" onClick={() => setOpen(false)}>
            {t("exportOptions.done")}
          </button>
        </footer>
      </div>
    </div>
  );
}
