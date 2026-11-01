// 全文朗读悬浮控制条（TDD-05 §9）：播放/暂停 · 停止 · 进度（第几句/共几句）· 语速。
//
// 仅当编辑器的朗读会话进行中时由 VaultEditor 渲染。进度用原生 `<progress>`（与 ProcessingProgress
// 同基线，无内联样式），语速下拉写回 `speech.rate` 设置并即时应用到后续句（`tts_set_rate`）。

import { useTranslation } from "react-i18next";

import { PauseIcon, PlayIcon, StopIcon } from "../../components/ui/Icon";
import type { TtsStateChanged } from "../../ipc/bindings";
import { SelectBox } from "../../components/ui/SelectBox";

/** 语速预设（百分比；100 = 常速）。展示为 `×` 倍率（数值，非文案）。 */
const RATE_PRESETS = [75, 100, 125, 150, 200] as const;

type ReadAloudControlsProps = {
  state: TtsStateChanged;
  ratePct: number;
  onPauseResume: () => void;
  onStop: () => void;
  onRateChange: (ratePct: number) => void;
};

export function ReadAloudControls({
  state,
  ratePct,
  onPauseResume,
  onStop,
  onRateChange,
}: ReadAloudControlsProps) {
  const { t } = useTranslation("editor");
  const speaking = state.status === "speaking";
  const current = Math.min(state.index + 1, state.total);

  return (
    <div className="luna-read-aloud" role="group" aria-label={t("read.controls")}>
      <button
        type="button"
        className="luna-read-aloud__btn"
        onClick={onPauseResume}
        aria-label={speaking ? t("read.pause") : t("read.resume")}
        title={speaking ? t("read.pause") : t("read.resume")}
      >
        {speaking ? <PauseIcon size={15} /> : <PlayIcon size={15} />}
      </button>
      <button
        type="button"
        className="luna-read-aloud__btn"
        onClick={onStop}
        aria-label={t("read.stop")}
        title={t("read.stop")}
      >
        <StopIcon size={15} />
      </button>
      <progress
        className="luna-read-aloud__progress"
        value={current}
        max={Math.max(state.total, 1)}
      />
      <span className="luna-read-aloud__count">
        {current} / {state.total}
      </span>
      <SelectBox
        size="sm"
        className="luna-read-aloud__rate"
        value={ratePct}
        ariaLabel={t("read.speed")}
        title={t("read.speed")}
        onChange={(event) => onRateChange(Number(event.currentTarget.value))}
      >
        {RATE_PRESETS.map((preset) => (
          <option key={preset} value={preset}>
            {preset / 100}×
          </option>
        ))}
      </SelectBox>
    </div>
  );
}
