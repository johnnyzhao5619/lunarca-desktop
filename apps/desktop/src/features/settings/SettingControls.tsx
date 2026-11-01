/**
 * 设置控件族：按注册表下发的**控件契约**（`SettingSpecDto.control`）分发渲染。
 *
 * 关键约定（ADR-003）：选项集合、取值范围、步长、单位、路径语义全部来自 Rust；本文件
 * 只负责「长什么样、怎么交互」。此前这里靠 `settingOptions(key)` 在前端硬编码了 20 余个
 * 键的选项表，与后端校验各说各话——那份表已删除。
 */

import { useEffect, useRef, useState } from "react";
import { useSettingDraft } from "./use-setting-draft";
import { cssVariables } from "../../lib/css-variables";
import { SelectBox } from "../../components/ui/SelectBox";
import { useTranslation } from "react-i18next";
import {
  commands,
  type AudioInputDeviceDto,
  type SettingControlDto,
  type SettingSpecDto,
  type TextPreviewDto,
  type SettingValueDto,
  type ToggleOptionDto,
} from "../../ipc/bindings";
import {
  ChevronDownIcon,
  ChevronUpIcon,
  FileIcon,
  FolderIcon,
  LockIcon,
} from "../../components/ui/Icon";
import { IconButton } from "../../components/ui/IconButton";
import {
  displayKey,
  DOUBLE_TAP_PREFIX,
  isReservedEditingCombo,
  parseShortcutValue,
} from "../../lib/shortcut-display";
import { isModifierKey, mainKeyOf } from "../../lib/shortcut-keys";
import { isImeKeyEvent } from "../../lib/ime-key-event";

/** 成功时返回 Rust 已归一化的权威值；失败为 null。 */
export type SetValue = (key: string, value: SettingValueDto) => Promise<SettingValueDto | null>;

export interface ControlProps {
  spec: SettingSpecDto;
  setValue: SetValue;
  advancedMode?: boolean;
  /** 该项当前的写入错误（若有）——控件据此进入错误态（描边 + aria-invalid）。 */
  invalid?: boolean;
  /**
   * 清掉这一行的写入错误。用户重新开始录制时必须清：上一句判语讲的是一个没被采纳、界面上
   * 也看不到的组合，留在屏幕上就成了对着当前这个值的一句假话。
   */
  clearError?: () => void;
}

/** 字符串取值（非字符串型返回空串——类型由 Rust 保证，此处只做防御性读取）。 */
export function strValue(spec: SettingSpecDto | undefined): string {
  return spec?.current_value.kind === "str" ? spec.current_value.value : "";
}

function intValue(spec: SettingSpecDto): number {
  return spec.current_value.kind === "int" ? spec.current_value.value : 0;
}

/** 开关：原生 checkbox 的语义 + 可上手的开关观感（role=switch，键盘可达）。 */
function ToggleControl({ spec, setValue, invalid }: ControlProps) {
  const { t } = useTranslation("settings");
  const checked = spec.current_value.kind === "bool" && spec.current_value.value;
  return (
    <button
      type="button"
      role="switch"
      aria-label={t(spec.i18n_key)}
      aria-invalid={invalid}
      aria-checked={checked}
      className="luna-switch"
      onClick={() => {
        void setValue(spec.key, { kind: "bool", value: !checked });
      }}
    >
      <span className="luna-switch__track" aria-hidden="true">
        <span className="luna-switch__thumb" />
      </span>
    </button>
  );
}

/**
 * 选项标签解析：优先该项专属文案（`option.<key>.<value>`），回落语言表（`common:lang.*`），
 * 再回落原始取值。语言型设置无需再维护一张"哪些键是语言"的名单（此前那张表本身就是债）。
 */
function useOptionLabel(spec: SettingSpecDto) {
  const { t } = useTranslation("settings");
  return (option: string): string => {
    const specific = t(`option.${spec.key}.${option}`, { defaultValue: "" });
    if (specific) return specific;
    const lang = t(`common:lang.${option}`, { defaultValue: "" });
    return lang || option;
  };
}

/**
 * 分段控件：少量短选项并排常显，当前档一眼可见（ARIA radiogroup：方向键换档、Home/End 到两端）。
 * 与下拉是**同一个契约的两种形态**——选哪一种由注册表 `control.style` 声明（`SelectStyle`），
 * 前端不按文案长度猜：同一项的中英文标签长度不同，按字数猜会让同一个设置在两种语言下
 * 长成两种控件。
 */
function SegmentedControl({
  spec,
  setValue,
  options,
  invalid,
}: ControlProps & { options: string[] }) {
  const { t } = useTranslation("settings");
  const label = useOptionLabel(spec);
  const current = strValue(spec);

  const handleKeyDown = (event: React.KeyboardEvent<HTMLButtonElement>, index: number) => {
    let nextIndex: number;
    switch (event.key) {
      case "ArrowRight":
      case "ArrowDown":
        nextIndex = (index + 1) % options.length;
        break;
      case "ArrowLeft":
      case "ArrowUp":
        nextIndex = (index - 1 + options.length) % options.length;
        break;
      case "Home":
        nextIndex = 0;
        break;
      case "End":
        nextIndex = options.length - 1;
        break;
      default:
        return;
    }
    event.preventDefault();
    const nextOption = options[nextIndex];
    if (nextOption === undefined) return;
    void setValue(spec.key, { kind: "str", value: nextOption });
    const radios = event.currentTarget.parentElement?.querySelectorAll<HTMLButtonElement>(
      '[role="radio"]',
    );
    radios?.[nextIndex]?.focus();
  };

  return (
    <div
      className="luna-segmented-control"
      role="radiogroup"
      aria-label={t(spec.i18n_key)}
      aria-invalid={invalid}
    >
      {options.map((option, index) => {
        const selected = option === current;
        return (
          <button
            key={option}
            type="button"
            role="radio"
            aria-checked={selected}
            tabIndex={selected ? 0 : -1}
            className="luna-segmented-control__item"
            data-active={selected}
            onClick={() => {
              void setValue(spec.key, { kind: "str", value: option });
            }}
            onKeyDown={(event) => handleKeyDown(event, index)}
          >
            {label(option)}
          </button>
        );
      })}
    </div>
  );
}

function SelectControl({
  spec,
  setValue,
  options,
  style,
  invalid,
}: ControlProps & { options: string[]; style: string }) {
  const { t } = useTranslation("settings");
  const label = useOptionLabel(spec);
  const current = strValue(spec);

  if (style === "segmented") {
    return <SegmentedControl spec={spec} setValue={setValue} options={options} invalid={invalid} />;
  }

  return (
    <SelectBox
      className="luna-settings-field__select"
      ariaLabel={t(spec.i18n_key)}
      value={current}
      aria-invalid={invalid}
      onChange={(event) => {
        void setValue(spec.key, { kind: "str", value: event.target.value });
      }}
    >
      {options.map((option) => (
        <option key={option} value={option}>
          {label(option)}
        </option>
      ))}
    </SelectBox>
  );
}

/** 单位后缀（i18n；`none` 不显示）。 */
function useUnitLabel() {
  const { t } = useTranslation("settings");
  return (unit: string, count?: number): string => (unit === "none" ? "" : t(`unit.${unit}`, { count, defaultValue: "" }));
}

/** 整数：滑块（连续量）或步进器（精确计数），范围/步长/单位全由注册表声明。 */
function NumberControl({
  spec,
  setValue,
  invalid,
  min,
  max,
  step,
  unit,
  displayDivisor,
  displayUnit,
  style,
}: ControlProps & {
  min: number;
  max: number;
  step: number;
  unit: string;
  displayDivisor: number;
  displayUnit: string;
  style: string;
}) {
  const { t } = useTranslation("settings");
  const labelUnit = useUnitLabel();
  const current = intValue(spec);
  const { draft, setDraft, adopt, version } = useSettingDraft(current);
  const unitLabel = labelUnit(style === "slider" ? displayUnit : unit, draft / displayDivisor);
  const commit = async () => {
    if (draft === current) return;
    const submittedVersion = version.current;
    const saved = await setValue(spec.key, { kind: "int", value: draft });
    if (saved?.kind === "int") adopt(saved.value, submittedVersion);
  };

  if (style === "slider") {
    return (
      <div className="luna-settings-number" data-style="slider">
        <input
          type="range"
          aria-label={t(spec.i18n_key)}
          className="luna-range luna-settings-number__slider"
          min={min}
          max={max}
          step={step}
          value={draft}
          aria-valuetext={`${draft / displayDivisor}${unitLabel}`}
          aria-invalid={invalid}
          style={cssVariables({
            "--range-fill": `${max > min ? ((draft - min) / (max - min)) * 100 : 0}%`,
          })}
          onChange={(event) => {
            setDraft(Number(event.target.value));
          }}
          // 拖动过程中只更新本地草稿，松手/键盘确认才落盘（避免一次拖拽写几十次配置文件）。
          onPointerUp={() => {
            void commit();
          }}
          onKeyUp={() => {
            void commit();
          }}
          onBlur={() => {
            void commit();
          }}
        />
        <output className="luna-settings-number__value">
          {draft / displayDivisor}
          {unitLabel}
        </output>
      </div>
    );
  }

  return (
    <div className="luna-settings-number" data-style="stepper">
      <input
        type="number"
        aria-label={t(spec.i18n_key)}
        className="luna-settings-field__input"
        min={min}
        max={max}
        step={step}
        value={draft}
        aria-invalid={invalid}
        onChange={(event) => {
          setDraft(Number(event.target.value));
        }}
        onBlur={() => {
          // **不在前端把越界值夹回范围内**：什么值合法是注册表声明、Rust 裁决的事
          // （ADR-003）。此前这里 `clampToStep(draft, min, max, step)` 一钳，用户输的
          // 9999 就静默变成 200 落盘——既没人告诉他上限是多少，也让 Rust 那条完整的
          // `OutOfRange → SETTING_RANGE → error.range` 链**永远走不到**（红边与那句
          // 判语是死代码，真机 2026-09-10 复现）。照原样送出去，越界由行内红字如实说。
          void commit();
        }}
        onKeyDown={(event) => {
          if (event.key === "Enter" && !isImeKeyEvent(event)) {
            event.currentTarget.blur();
          }
        }}
      />
      {unitLabel ? <span className="luna-settings-number__unit">{unitLabel}</span> : null}
    </div>
  );
}

/** 小数（字符串存储）：范围与步长同样由注册表声明，越界由 Rust 拒绝并就地提示。 */
function DecimalControl({
  spec,
  setValue,
  invalid,
  min,
  max,
  step,
  unit,
}: ControlProps & {
  min: number | null;
  max: number | null;
  step: number | null;
  unit: string;
}) {
  const { t } = useTranslation("settings");
  const unitLabel = useUnitLabel()(unit);
  const current = strValue(spec);
  const { draft, setDraft, adopt, version } = useSettingDraft(current);
  return (
    <div className="luna-settings-number" data-style="stepper">
      <input
        type="number"
        aria-label={t(spec.i18n_key)}
        className="luna-settings-field__input"
        min={min ?? undefined}
        max={max ?? undefined}
        step={step ?? undefined}
        value={draft}
        aria-invalid={invalid}
        onChange={(event) => {
          setDraft(event.target.value);
        }}
        onBlur={() => {
          if (draft === current) return;
          const submittedVersion = version.current;
          void setValue(spec.key, { kind: "str", value: draft }).then((saved) => {
            if (saved?.kind === "str") adopt(saved.value, submittedVersion);
          });
        }}
        onKeyDown={(event) => {
          if (event.key === "Enter" && !isImeKeyEvent(event)) {
            event.currentTarget.blur();
          }
        }}
      />
      {unitLabel ? <span className="luna-settings-number__unit">{unitLabel}</span> : null}
    </div>
  );
}

/**
 * 文本设置的实时预览（注册表声明 `control.preview`，2026-10-08 C47）：边输入边问 Rust
 * 「这份模板会渲染成什么」，同一个渲染函数也是导出时用的那一个。预览是只读的——
 * 不写设置、不替用户校验；模板不合法时不显示预览，原因由写入那条路径就地说。
 */
function TextPreview({ preview, draft }: { preview: TextPreviewDto; draft: string }) {
  const { t } = useTranslation("settings");
  const [rendered, setRendered] = useState<string | null>(null);
  useEffect(() => {
    let active = true;
    switch (preview) {
      case "export_filename":
        void commands
          .previewExportFilename(draft, t("export.preview_sample_name"))
          .then((result) => {
            if (active) setRendered(result);
          });
        break;
    }
    return () => {
      active = false;
    };
  }, [preview, draft, t]);
  if (rendered === null) return null;
  return (
    <p className="luna-settings-field__preview" aria-live="polite">
      {t("export.preview", { name: rendered })}
    </p>
  );
}

function TextControl({ spec, setValue, invalid }: ControlProps) {
  const { t } = useTranslation("settings");
  const current = strValue(spec);
  const { draft, setDraft, adopt, version } = useSettingDraft(current);
  const preview = spec.control.kind === "text" ? spec.control.preview : null;
  const multiline = spec.control.kind === "text" && spec.control.multiline;
  const commit = () => {
    if (draft === current) return;
    const submittedVersion = version.current;
    void setValue(spec.key, { kind: "str", value: draft }).then((saved) => {
      if (saved?.kind === "str") adopt(saved.value, submittedVersion);
    });
  };
  // 一段话（注册表声明 `multiline`）：多行框，回车就是换行，离开时保存；字数上限由 Rust 判。
  if (multiline) {
    return (
      <textarea
        aria-label={t(spec.i18n_key)}
        className="luna-settings-field__textarea"
        rows={4}
        value={draft}
        aria-invalid={invalid}
        onChange={(event) => {
          setDraft(event.target.value);
        }}
        onBlur={commit}
      />
    );
  }
  const input = (
    <input
      type="text"
      aria-label={t(spec.i18n_key)}
      className="luna-settings-field__input"
      value={draft}
      aria-invalid={invalid}
      onChange={(event) => {
        setDraft(event.target.value);
      }}
      onBlur={() => {
        if (draft === current) return;
        const submittedVersion = version.current;
        void setValue(spec.key, { kind: "str", value: draft }).then((saved) => {
          // 只回显 Rust 返回的事实。尤其当规范化结果等于编辑前的 current 时，父组件的
          // `[current]` 不会变化，不能只靠 effect 把省略协议的草稿改回完整 URL。
          if (saved?.kind === "str") adopt(saved.value, submittedVersion);
        });
      }}
      onKeyDown={(event) => {
        if (event.key === "Enter" && !isImeKeyEvent(event)) {
          event.currentTarget.blur();
        }
      }}
    />
  );
  if (preview === null) return input;
  return (
    <div className="luna-settings-text">
      {input}
      <TextPreview preview={preview} draft={draft} />
    </div>
  );
}

/**
 * 路径四态：
 * - `vault_root`：换库要**重新绑定**（建目录/挂媒体服务/装监听/重扫索引），故走 `select_vault`，
 *   而不是往设置里写一个字符串（那会留下"路径改了库没换"的半吊子状态）。
 * - `absolute_dir`：系统目录选择器 + 「用默认位置」清空。
 * - `absolute_file`：系统**文件**选择器（外部工具可执行文件）+ 「自动查找」清空。
 *   与 `absolute_dir` 分立是因为原生对话框不同类——目录选择器里选不中一个可执行文件。
 * - `vault_relative_dir`：库内相对路径文本框（绝对路径/`..` 由 Rust 拒绝并就地提示）。
 */
function PathControl({
  spec,
  setValue,
  invalid,
  pathKind,
  onVaultChanged,
}: ControlProps & { pathKind: string; onVaultChanged: () => void }) {
  const { t } = useTranslation("settings");
  const current = strValue(spec);

  if (pathKind === "vault_relative_dir") {
    return <TextControl spec={spec} setValue={setValue} invalid={invalid} />;
  }

  const browse = async () => {
    // 文件类走文件选择器：可执行文件在目录选择器里根本选不中。
    const picked =
      pathKind === "absolute_file" ? await commands.pickFile() : await commands.pickFolder(null);
    if (picked.status !== "ok" || !picked.data) return;
    if (pathKind === "vault_root") {
      const bound = await commands.selectVault(picked.data);
      if (bound.status === "ok") {
        onVaultChanged();
      }
      return;
    }
    await setValue(spec.key, { kind: "str", value: picked.data });
  };

  return (
    <div className="luna-settings-path">
      <span
        className="luna-settings-path__value"
        title={current || undefined}
        aria-invalid={invalid}
      >
        {current || t("path.unset")}
      </span>
      <button
        type="button"
        className="luna-btn luna-btn--secondary luna-settings-path__browse"
        onClick={() => {
          void browse();
        }}
      >
        {pathKind === "absolute_file" ? <FileIcon size={14} /> : <FolderIcon size={14} />}
        <span>{t("path.browse")}</span>
      </button>
      {(pathKind === "absolute_dir" || pathKind === "absolute_file") && current ? (
        <button
          type="button"
          className="luna-settings-field__reset"
          onClick={() => {
            void setValue(spec.key, { kind: "str", value: "" });
          }}
        >
          {/* 清空的含义随语义变：目录 = 回到默认位置，工具文件 = 回到自动查找。 */}
          {pathKind === "absolute_file" ? t("path.auto_detect") : t("path.use_default")}
        </button>
      ) : null}
    </div>
  );
}

/** 音频输入设备：候选来自系统枚举（空 = 跟随系统默认，PRD-09-040）。 */
function AudioInputDeviceControl({ spec, setValue }: ControlProps) {
  const { t } = useTranslation("settings");
  const [devices, setDevices] = useState<AudioInputDeviceDto[]>([]);
  const current = strValue(spec);

  useEffect(() => {
    void commands.listAudioInputDevices().then(setDevices);
  }, []);

  const known = devices.some((device) => device.name === current);

  return (
    <SelectBox
      className="luna-settings-field__select"
      ariaLabel={t(spec.i18n_key)}
      value={current}
      onChange={(event) => {
        void setValue(spec.key, { kind: "str", value: event.target.value });
      }}
    >
      <option value="">{t("recording.input_device_default")}</option>
      {/* 设备已拔出但设置仍指向它：保留该项并如实显示，不静默改写用户的选择。 */}
      {!known && current ? <option value={current}>{current}</option> : null}
      {devices.map((device) => (
        <option key={device.name} value={device.name}>
          {device.name}
        </option>
      ))}
    </SelectBox>
  );
}

/**
 * 快捷键控件：录制器 + 触发方式（按一次 / 双击）。
 *
 * 触发方式对任意快捷键都可选（全局键由 Rust 事件 tap 判双击，应用内键由命令注册表判），
 * 未绑定时也能先选好、录到键后一并写入——不必先录一次再切一次（两次写入两次重注册）。
 * 保留编辑键（⌘C 等）只能双击，「按一次」禁用并说明理由。
 *
 * 录制约定与 macOS 系统设置 / Raycast 一致：Esc 取消并保留旧值，Delete / Backspace 清除，
 * 点到别处取消。录制期间全局键整体收起（`set_shortcut_capture`），出路全走 effect 清理。
 */
function ShortcutControl({ spec, setValue, invalid, clearError, advancedMode }: ControlProps) {
  const { t } = useTranslation("settings");
  const value = strValue(spec);
  const parsed = parseShortcutValue(value);
  const bound = parsed.parts.length > 0;
  const reserved = bound && isReservedEditingCombo(parsed.parts);
  const [pendingDoubleTap, setPendingDoubleTap] = useState(false);
  const doubleTap = bound ? parsed.doubleTap || reserved : pendingDoubleTap;
  const [isRecording, setIsRecording] = useState(false);
  /** 上一下按的是个认不出的键（死键 / 输入法处理中）——录制继续，只是换一句提示。 */
  const [unknownKey, setUnknownKey] = useState(false);
  const containerRef = useRef<HTMLDivElement>(null);
  const [recordingKeys, setRecordingKeys] = useState<{
    meta: boolean;
    ctrl: boolean;
    alt: boolean;
    shift: boolean;
    key: string;
  }>({ meta: false, ctrl: false, alt: false, shift: false, key: "" });

  useEffect(() => {
    if (!isRecording) return;

    // 录制期间先把全局键整体收起来：系统级热键与事件 tap 都不经过 webview，不收起的话
    // 按到已被占用的组合会当场唤起占用者。装回去挂在本 effect 的清理上，每条出路都会走到。
    void commands.setShortcutCapture(true);

    const handleKeyDown = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();

      const key = e.key;
      const noModifiers = !e.metaKey && !e.ctrlKey && !e.altKey && !e.shiftKey;
      if (noModifiers && key === "Escape") {
        setIsRecording(false);
        return;
      }
      if (noModifiers && (key === "Backspace" || key === "Delete")) {
        void setValue(spec.key, { kind: "str", value: "" });
        setIsRecording(false);
        return;
      }

      const isModifier = isModifierKey(key);
      // 认不出的键（死键、输入法处理中的键、异形键盘的私有键）当场说认不出，不写进设置。
      const keyName = isModifier ? "" : (mainKeyOf(e) ?? "");
      if (!isModifier && !keyName) {
        setUnknownKey(true);
        return;
      }
      setUnknownKey(false);

      const state = {
        meta: e.metaKey,
        ctrl: e.ctrlKey,
        alt: e.altKey,
        shift: e.shiftKey,
        key: keyName,
      };
      setRecordingKeys(state);

      if (keyName) {
        const parts: string[] = [];
        if (state.meta || state.ctrl) parts.push("cmd_or_ctrl");
        if (state.shift) parts.push("shift");
        if (state.alt) parts.push("alt");
        parts.push(keyName);

        const combo = parts.join("+");
        const useDoubleTap = doubleTap || isReservedEditingCombo(parts);
        void setValue(spec.key, {
          kind: "str",
          value: useDoubleTap ? `${DOUBLE_TAP_PREFIX}${combo}` : combo,
        });
        setIsRecording(false);
      }
    };

    const handleClickOutside = (e: MouseEvent) => {
      if (containerRef.current && !containerRef.current.contains(e.target as Node)) {
        setIsRecording(false);
      }
    };

    window.addEventListener("keydown", handleKeyDown, true);
    window.addEventListener("mousedown", handleClickOutside);
    return () => {
      window.removeEventListener("keydown", handleKeyDown, true);
      window.removeEventListener("mousedown", handleClickOutside);
      void commands.setShortcutCapture(false);
    };
  }, [isRecording, spec.key, setValue, doubleTap]);

  const startRecording = () => {
    setUnknownKey(false);
    clearError?.();
    setRecordingKeys({
      meta: false,
      ctrl: false,
      alt: false,
      shift: false,
      key: "",
    });
    setIsRecording(true);
  };

  const selectMode = (next: boolean) => {
    if (next === doubleTap) return;
    if (!bound) {
      setPendingDoubleTap(next);
      return;
    }
    if (reserved && !next) return;
    const combo = parsed.parts.join("+");
    void setValue(spec.key, {
      kind: "str",
      value: next ? `${DOUBLE_TAP_PREFIX}${combo}` : combo,
    });
  };

  const recordingParts: string[] = [];
  if (recordingKeys.meta || recordingKeys.ctrl) recordingParts.push("cmd_or_ctrl");
  if (recordingKeys.shift) recordingParts.push("shift");
  if (recordingKeys.alt) recordingParts.push("alt");
  if (recordingKeys.key) recordingParts.push(recordingKeys.key);
  const shownParts = isRecording ? recordingParts : parsed.parts;

  const modeOptions: Array<{ doubleTap: boolean; label: string; hint?: string }> = [
    {
      doubleTap: false,
      label: t("shortcuts.mode_single"),
      hint: reserved ? t("shortcuts.double_tap_locked") : undefined,
    },
    { doubleTap: true, label: t("shortcuts.mode_double"), hint: t("shortcuts.mode_double_hint") },
  ];

  return (
    <div className="luna-shortcut-control" ref={containerRef}>
      <div
        className="luna-shortcut-recorder"
        data-recording={isRecording || undefined}
        data-invalid={invalid}
        role="button"
        aria-label={t(spec.i18n_key)}
        tabIndex={isRecording ? -1 : 0}
        aria-live="polite"
        onClick={isRecording ? undefined : startRecording}
        onKeyDown={(e) => {
          if (!isRecording && (e.key === "Enter" || e.key === " ")) {
            e.preventDefault();
            startRecording();
          }
        }}
        title={isRecording ? t("shortcuts.recording_tip") : t("shortcuts.click_to_record")}
      >
        {shownParts.length > 0 ? (
          <div className="luna-shortcut-recorder__keys">
            {shownParts.map((part, idx) => (
              <kbd key={idx}>{displayKey(part)}</kbd>
            ))}
            {!isRecording && doubleTap ? (
              <span className="luna-shortcut-recorder__times" aria-label={t("shortcuts.mode_double")}>
                ×2
              </span>
            ) : null}
          </div>
        ) : isRecording ? (
          <span className="luna-shortcut-recorder__placeholder">
            {unknownKey ? t("shortcuts.unsupported_key") : t("shortcuts.press_keys")}
          </span>
        ) : (
          // 未绑定也是**一枚键帽**，不是一句斜体灰字：同一列里两种长相时，用户读到的
          // 不是"这一格是空的"，而是"这一行和别的行不是一个东西"（真机症状 ⑤）。
          // 虚线描边 + 常规字重把它与真键区分开，形状、行高与左缘则与真键完全一致。
          <kbd className="luna-shortcut-recorder__empty">{t("shortcuts.unbound")}</kbd>
        )}
      </div>
      {(advancedMode || (spec.control.kind === "shortcut" && spec.control.trigger_mode_visibility === "standard")) ? <div
        className="luna-segmented-control luna-shortcut-control__mode"
        role="radiogroup"
        aria-label={t("shortcuts.trigger_mode")}
      >
        {modeOptions.map((option) => {
          const selected = option.doubleTap === doubleTap;
          const disabled = reserved && !option.doubleTap;
          return (
            <button
              key={String(option.doubleTap)}
              type="button"
              role="radio"
              aria-checked={selected}
              tabIndex={selected ? 0 : -1}
              className="luna-segmented-control__item"
              data-active={selected}
              disabled={disabled}
              title={option.hint}
              onClick={() => selectMode(option.doubleTap)}
            >
              {option.label}
            </button>
          );
        })}
      </div> : null}
    </div>
  );
}

/** 有序多选里一行上的控件（焦点保留按这个 key 找回来）。 */
type OrderedRowControl = "up" | "down" | "toggle";

/**
 * 有序多选（`OrderedMultiSelect`，TDD-05 §7.5a）：**全集逐行列出**——已选在前、按序；未选灰在后。
 * 每行一枚开关（开 = 加到已选末尾，关 = 移除），上移/下移只在已选行可用。
 *
 * **与「有序开关列表」同一套行形态**（序号 / 名字 / 开关 / 上下）：两个列表控件在同一个
 * 设置分组里上下相邻，此前一个是"下拉替换 + 增项下拉"、一个是开关——同一件事两种手势，
 * 真机上第一眼就分不清哪个能改什么。
 *
 * **增删与重排共用同一条"提交完整新数组"的通路**，不为增删另开命令或增量协议。
 * 整数组提交同时消掉"四项已满或只剩一种类别时先删被拒、先加也被拒"的死路——最终合法性
 * 只由 Rust 裁决，前端交出去的永远是一个完整候选数组。上下限来自注册表下发的
 * `min` / `max`（本项 2 / 4），不在前端写死：到上限时未选行的开关禁用，到下限时已选行的
 * 开关禁用——禁用而不是消失，消失会让人以为这一项没有开关。
 *
 * **不做拖拽**（或者说：不只做拖拽）：上移/下移是按钮，键盘与读屏天然可用；纯拖拽的
 * 重排对键盘用户等于没有这个功能（PRD-00 §7）。增删/重排后一律由 `role="status"`
 * 报出结果，焦点留在所操作项上——否则读屏用户按下"上移"后什么反馈都没有。
 *
 * **前端不判合法性**：条数、重复、**类目下限（写作与导入各至少一项）**全部由 Rust
 * 裁决。类目尤其不能在这里算：那条规则会在 Rust 与前端各有一份，而前端那份必然先陈旧。
 * 被拒时行级 `role="alert"` 已经把理由念出来（`SettingRow` 的 `.luna-field-error`），
 * 这里只把乐观状态**撤掉**，不再重复念一遍。
 */
function OrderedMultiSelectControl({
  spec,
  setValue,
  invalid,
  options,
  min,
  max,
}: ControlProps & { options: string[]; min: number; max: number }) {
  const { t } = useTranslation("settings");
  const label = useOptionLabel(spec);
  const selected = spec.current_value.kind === "string_list" ? spec.current_value.value : [];
  const unselected = options.filter((option) => !selected.includes(option));
  const [status, setStatus] = useState("");
  const buttonsRef = useRef(new Map<string, HTMLButtonElement>());
  /**
   * 待落位的焦点。`expected` 是"提交的那一版数组"——只有当前值真的变成它之后才落焦点。
   *
   * 写入是**异步经 Rust 回来的**（`set_setting` → `settings://changed` → 重读），
   * 提交那一刻列表还是旧的。不等这一版就落焦点，等于在旧布局上落一次、新布局到达后
   * 又丢掉。
   */
  const [pendingFocus, setPendingFocus] = useState<{
    expected: string;
    option: string;
    control: OrderedRowControl;
  } | null>(null);
  const selectedKey = selected.join(" ");

  const registerButton = (option: string, control: OrderedRowControl) => {
    return (node: HTMLButtonElement | null) => {
      const key = `${option} ${control}`;
      if (node) {
        buttonsRef.current.set(key, node);
      } else {
        buttonsRef.current.delete(key);
      }
    };
  };

  useEffect(() => {
    if (!pendingFocus || pendingFocus.expected !== selectedKey) {
      return;
    }
    setPendingFocus(null);
    const at = (control: OrderedRowControl) =>
      buttonsRef.current.get(`${pendingFocus.option} ${control}`);
    // 移到首/末位时那颗按钮当场被置灰，焦点会掉回 `<body>`——键盘用户下一次 Tab
    // 得从页首重走一遍。故落到**仍可用的对侧按钮**上：位置变了，手还在这一行。
    const order: OrderedRowControl[] =
      pendingFocus.control === "up"
        ? ["up", "down", "toggle"]
        : pendingFocus.control === "down"
          ? ["down", "up", "toggle"]
          : ["toggle", "up", "down"];
    for (const control of order) {
      const node = at(control);
      if (node && !node.disabled) {
        node.focus();
        return;
      }
    }
  }, [pendingFocus, selectedKey]);

  /**
   * 提交一个完整数组，并在**知道结果之后**才播报。
   *
   * 不 await 就乐观播报的表现是：Rust 拒了（比如删到只剩写作入口），读屏却已经说过
   * "已移除，现在共 2 项"——而屏幕上一项都没少。
   */
  const commit = async (
    next: string[],
    announcement: string,
    focus: { option: string; control: OrderedRowControl },
  ) => {
    const saved = await setValue(spec.key, { kind: "string_list", value: next });
    if (!saved) {
      setStatus("");
      setPendingFocus(null);
      return;
    }
    setStatus(announcement);
    setPendingFocus({ expected: next.join(" "), ...focus });
  };

  const move = (index: number, delta: number) => {
    const target = index + delta;
    if (target < 0 || target >= selected.length) {
      return;
    }
    const option = selected[index];
    const next = [...selected];
    [next[index], next[target]] = [next[target], next[index]];
    void commit(
      next,
      t("orderedMultiSelect.moved", {
        label: label(option),
        position: target + 1,
        total: next.length,
      }),
      { option, control: delta < 0 ? "up" : "down" },
    );
  };

  // 开 = 追加到已选末尾（用户可以接着上移它）；关 = 移除，回到未选段——两种情况下那一行
  // 的开关都还在（同一个 key），焦点原地不动。
  const add = (candidate: string) => {
    const next = [...selected, candidate];
    void commit(
      next,
      t("orderedMultiSelect.added", {
        label: label(candidate),
        position: next.length,
        total: next.length,
      }),
      { option: candidate, control: "toggle" },
    );
  };

  const remove = (index: number) => {
    const option = selected[index];
    const next = selected.filter((_, at) => at !== index);
    void commit(
      next,
      t("orderedMultiSelect.removed", { label: label(option), total: next.length }),
      { option, control: "toggle" },
    );
  };

  const atMax = selected.length >= max;
  const atMin = selected.length <= min;
  const rows = [
    ...selected.map((option, index) => ({ option, index })),
    ...unselected.map((option) => ({ option, index: -1 })),
  ];

  return (
    <div className="luna-ordered-select" data-invalid={invalid || undefined}>
      <ol className="luna-ordered-select__chosen">
        {rows.map(({ option, index }) => {
          const picked = index >= 0;
          const name = label(option);
          return (
            <li
              key={option}
              className="luna-ordered-select__row"
              data-unselected={!picked || undefined}
            >
              <span className="luna-ordered-select__position" aria-hidden="true">
                {picked ? index + 1 : ""}
              </span>
              <span className="luna-ordered-select__label">{name}</span>
              {/* 删项只影响这一处投影；「添加内容」的完整目录不受影响（PRD-07-003）。 */}
              <button
                ref={registerButton(option, "toggle")}
                type="button"
                role="switch"
                aria-checked={picked}
                aria-label={t("orderedMultiSelect.toggle", { label: name })}
                className="luna-switch"
                disabled={picked ? atMin : atMax}
                onClick={() => (picked ? remove(index) : add(option))}
              >
                <span className="luna-switch__track" aria-hidden="true">
                  <span className="luna-switch__thumb" />
                </span>
              </button>
              <IconButton
                ref={registerButton(option, "up")}
                label={t("orderedMultiSelect.moveUp", { label: name })}
                icon={<ChevronUpIcon size={13} />}
                disabled={!picked || index === 0}
                onClick={() => move(index, -1)}
              />
              <IconButton
                ref={registerButton(option, "down")}
                label={t("orderedMultiSelect.moveDown", { label: name })}
                icon={<ChevronDownIcon size={13} />}
                disabled={!picked || index === selected.length - 1}
                onClick={() => move(index, 1)}
              />
            </li>
          );
        })}
      </ol>
      {/* 增删/重排的可听反馈：读屏用户按下按钮之后必须知道现在成了什么样。 */}
      <span className="luna-ordered-select__status" role="status">
        {status}
      </span>
    </div>
  );
}

/** 有序开关列表里一行上的控件（焦点保留按这个 key 找回来）。 */
type ToggleRowControl = "up" | "down" | "toggle";

interface ToggleRow {
  option: ToggleOptionDto;
  visible: boolean;
}

/**
 * 有序开关列表（`OrderedToggleList`，PRD-07-003 / TDD-05 §7.5a）：全集恒在场、每项一枚开关、
 * 锁定项以锁形替代开关且不可关；上移/下移重排。首个消费者是活动栏布局 `workspace.rail_layout`。
 *
 * **前端不解析值语义，只按下发的 `hidden_prefix` 读写 token**：行序 = 值里的顺序；值里
 * 没有的选项（新版本新加的空间）追加在末尾并视为显示——提交任何改动时 Rust 会把整份
 * 归一化到完整全集（ADR-087 ⑥），这里只是别让它在设置页里消失。
 *
 * **每次提交都是完整数组**（与有序多选同一条通路）；锁定项不渲染开关，即便被写成隐藏
 * 也由 Rust 归一化重新置为显示——前端不另判一遍。名字取自选项自带的名字键（`ns:key`），
 * 它指向该对象在别处已有的那一个名字，不为设置页另起标签。
 */
function OrderedToggleListControl({
  spec,
  setValue,
  invalid,
  options,
  hiddenPrefix,
  minVisible,
}: ControlProps & { options: ToggleOptionDto[]; hiddenPrefix: string; minVisible: number }) {
  const { t } = useTranslation("settings");
  const tokens = spec.current_value.kind === "string_list" ? spec.current_value.value : [];
  const tokensKey = tokens.join(" ");
  const byId = new Map(options.map((option) => [option.id, option]));
  const seen = new Set<string>();
  const rows: ToggleRow[] = [];
  for (const token of tokens) {
    const hidden = token.startsWith(hiddenPrefix);
    const id = hidden ? token.slice(hiddenPrefix.length) : token;
    const option = byId.get(id);
    if (!option || seen.has(id)) {
      continue;
    }
    seen.add(id);
    rows.push({ option, visible: !hidden || option.locked });
  }
  // 值里缺的项按 Rust 下发的默认可见性补回（与 Rust 归一化同一规则，ADR-162 ①）。
  for (const option of options) {
    if (!seen.has(option.id)) {
      rows.push({ option, visible: option.default_visible || option.locked });
    }
  }
  const label = (option: ToggleOptionDto) => t(option.label_key);
  /** 已到下限（创作看板至少一列，ADR-162）：显示着的那几项的开关关不掉——下限由 Rust 下发。 */
  const atMinimum = rows.filter((row) => row.visible).length <= minVisible;

  const [status, setStatus] = useState("");
  const buttonsRef = useRef(new Map<string, HTMLButtonElement>());
  /** 待落位的焦点——只有当前值真的变成提交的那一版之后才落（写入是异步经 Rust 回来的）。 */
  const [pendingFocus, setPendingFocus] = useState<{
    expected: string;
    id: string;
    control: ToggleRowControl;
  } | null>(null);

  const registerButton = (id: string, control: ToggleRowControl) => {
    return (node: HTMLButtonElement | null) => {
      const key = `${id} ${control}`;
      if (node) {
        buttonsRef.current.set(key, node);
      } else {
        buttonsRef.current.delete(key);
      }
    };
  };

  useEffect(() => {
    if (!pendingFocus || pendingFocus.expected !== tokensKey) {
      return;
    }
    setPendingFocus(null);
    // 移到首/末位时那颗按钮当场被置灰，焦点会掉回 `<body>`——落到仍可用的对侧按钮上。
    const order: ToggleRowControl[] =
      pendingFocus.control === "up"
        ? ["up", "down", "toggle"]
        : pendingFocus.control === "down"
          ? ["down", "up", "toggle"]
          : ["toggle", "up", "down"];
    for (const control of order) {
      const node = buttonsRef.current.get(`${pendingFocus.id} ${control}`);
      if (node && !node.disabled) {
        node.focus();
        return;
      }
    }
  }, [pendingFocus, tokensKey]);

  const serialize = (next: readonly ToggleRow[]) =>
    next.map((row) => (row.visible ? row.option.id : `${hiddenPrefix}${row.option.id}`));

  /** 提交完整数组，并在**知道结果之后**才播报（被拒时屏幕上什么都没变，就不该说"已"）。 */
  const commit = async (
    next: readonly ToggleRow[],
    announcement: string,
    focus: { id: string; control: ToggleRowControl },
  ) => {
    const value = serialize(next);
    const saved = await setValue(spec.key, { kind: "string_list", value });
    if (!saved) {
      setStatus("");
      setPendingFocus(null);
      return;
    }
    setStatus(announcement);
    setPendingFocus({ expected: value.join(" "), ...focus });
  };

  const move = (index: number, delta: number) => {
    const target = index + delta;
    if (target < 0 || target >= rows.length) {
      return;
    }
    const next = [...rows];
    [next[index], next[target]] = [next[target], next[index]];
    const row = rows[index];
    void commit(
      next,
      t("orderedToggleList.moved", {
        label: label(row.option),
        position: target + 1,
        total: next.length,
      }),
      { id: row.option.id, control: delta < 0 ? "up" : "down" },
    );
  };

  const toggle = (index: number) => {
    const row = rows[index];
    const next = rows.map((entry, at) =>
      at === index ? { ...entry, visible: !entry.visible } : entry,
    );
    void commit(
      next,
      t(row.visible ? "orderedToggleList.hidden" : "orderedToggleList.shown", {
        label: label(row.option),
      }),
      { id: row.option.id, control: "toggle" },
    );
  };

  return (
    <div className="luna-ordered-select" data-invalid={invalid || undefined}>
      <ol className="luna-ordered-select__chosen">
        {rows.map((row, index) => {
          const name = label(row.option);
          return (
            <li key={row.option.id} className="luna-ordered-select__row">
              <span className="luna-ordered-select__position" aria-hidden="true">
                {index + 1}
              </span>
              <span className="luna-ordered-select__label">{name}</span>
              {row.option.locked ? (
                // 锚点：可排序、不可关（PRD-07-003）。锁形替代开关，而不是一个点不动的灰开关——
                // 灰开关在说"暂时不行"，锁在说"就是这样"。
                <span
                  className="luna-ordered-select__lock"
                  role="img"
                  aria-label={t("orderedToggleList.locked", { label: name })}
                  title={t("orderedToggleList.locked", { label: name })}
                >
                  <LockIcon size={13} />
                </span>
              ) : (
                <button
                  ref={registerButton(row.option.id, "toggle")}
                  type="button"
                  role="switch"
                  aria-checked={row.visible}
                  aria-label={t("orderedToggleList.toggle", { label: name })}
                  className="luna-switch"
                  disabled={row.visible && atMinimum}
                  title={row.visible && atMinimum ? t("orderedToggleList.atMinimum", { count: minVisible }) : undefined}
                  onClick={() => toggle(index)}
                >
                  <span className="luna-switch__track" aria-hidden="true">
                    <span className="luna-switch__thumb" />
                  </span>
                </button>
              )}
              <IconButton
                ref={registerButton(row.option.id, "up")}
                label={t("orderedToggleList.moveUp", { label: name })}
                icon={<ChevronUpIcon size={13} />}
                disabled={index === 0}
                onClick={() => move(index, -1)}
              />
              <IconButton
                ref={registerButton(row.option.id, "down")}
                label={t("orderedToggleList.moveDown", { label: name })}
                icon={<ChevronDownIcon size={13} />}
                disabled={index === rows.length - 1}
                onClick={() => move(index, 1)}
              />
            </li>
          );
        })}
      </ol>
      <details className="luna-settings-preview">
        <summary>{t("preview.layout.title")}</summary>
        <p className="luna-settings-panel__hint">{t("preview.layout.hint")}</p>
        <ol className="luna-settings-preview__rail" aria-label={t("preview.layout.title")}>
          {rows.filter((row) => row.visible).map((row) => (
            <li key={row.option.id}>{label(row.option)}</li>
          ))}
        </ol>
      </details>
      {/* 重排/开关的可听反馈：读屏用户按下按钮之后必须知道现在成了什么样。 */}
      <span className="luna-ordered-select__status" role="status">
        {status}
      </span>
    </div>
  );
}

/** 控件分发：`control.kind` 是唯一判据（不再按 key 名硬编码分支）。 */
export function SettingControl({
  spec,
  setValue,
  advancedMode,
  invalid,
  clearError,
  onVaultChanged,
}: ControlProps & { onVaultChanged: () => void }) {
  const control: SettingControlDto = spec.control;
  switch (control.kind) {
    case "toggle":
      return <ToggleControl spec={spec} setValue={setValue} invalid={invalid} />;
    case "select":
      return (
        <SelectControl
          spec={spec}
          setValue={setValue}
          invalid={invalid}
          options={control.options}
          style={control.style}
        />
      );
    case "number":
      return (
        <NumberControl
          spec={spec}
          setValue={setValue}
          invalid={invalid}
          min={control.min}
          max={control.max}
          step={control.step}
          unit={control.unit}
          displayDivisor={control.display_divisor}
          displayUnit={control.display_unit}
          style={control.style}
        />
      );
    case "decimal":
      return (
        <DecimalControl
          spec={spec}
          setValue={setValue}
          invalid={invalid}
          min={control.min}
          max={control.max}
          step={control.step}
          unit={control.unit}
        />
      );
    case "path":
      return (
        <PathControl
          spec={spec}
          setValue={setValue}
          invalid={invalid}
          pathKind={control.path_kind}
          onVaultChanged={onVaultChanged}
        />
      );
    case "shortcut":
      return (
        <ShortcutControl
          spec={spec}
          advancedMode={advancedMode}
          setValue={setValue}
          invalid={invalid}
          clearError={clearError}
        />
      );
    case "model_assignment":
      // 模型分配槽由「AI 引擎 / 模型」面板独占（唯一写入口 `assign_model`），注册表把它们
      // 标为面板承载 → 不会渲染成设置行。契约仍要求穷尽处理这一支。
      return null;
    case "audio_input_device":
      return <AudioInputDeviceControl spec={spec} setValue={setValue} invalid={invalid} />;
    case "ordered_multi_select":
      return (
        <OrderedMultiSelectControl
          spec={spec}
          setValue={setValue}
          invalid={invalid}
          options={control.options}
          // `min` / `max` 与 number/decimal 一样必须透传：丢掉它们，增删按钮就没有
          // 上下限可依，控件只能在前端另写一份 2/4（TDD-05 §7.5a 明说上下限来自注册表）。
          min={control.min}
          max={control.max}
        />
      );
    case "ordered_toggle_list":
      return (
        <OrderedToggleListControl
          spec={spec}
          setValue={setValue}
          invalid={invalid}
          options={control.options}
          hiddenPrefix={control.hidden_prefix}
          minVisible={control.min_visible}
        />
      );
    case "text":
      return <TextControl spec={spec} setValue={setValue} invalid={invalid} />;
    default:
      return null;
  }
}
