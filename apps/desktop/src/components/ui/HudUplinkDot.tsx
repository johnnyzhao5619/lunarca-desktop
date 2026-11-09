import { useTranslation } from "react-i18next";
import { CloudUploadIcon } from "@lunarca/editor-core/Icon";

/**
 * 联网指示（ADR-038 ④ / ADR-111 ⑤⑴）：音频正在（或将会）离开本机。
 *
 * 表面级插槽——任何 HUD、任何形态（含沉浸字幕）都不许隐藏它。此前同一块 span 在
 * 录音 HUD / 实时字幕 / 语音速记各抄一份，类名还借用录音的 `.luna-rec-hud__net-dot`
 * （TDD-15 §3.3a：被第三处需要的东西不属于任何 feature）；而沉浸字幕把承载它的顶栏
 * 整个 `display: none`，指示随之消失。收编成组件后，「藏掉它」必须显式删掉一个挂载点，
 * 而不是顺手藏一个容器——`hud-density-guard` 反向锁定沉浸分支必须有本组件。
 *
 * 字形是**云上传**（2026-09-07）：此前是一枚 8px 警告色三角，在窗口栏里读作报警或
 * 下拉箭头，与「音频离开设备」这件事对不上。两态：`active`（采集中，实心底 + 警告色，
 * 文案「正在离开」）/ 非 active（空闲，描边 + 中性色，文案「说话时将交给…」）——语音 HUD
 * 空闲时也挂着它，此前空闲态 aria 照样说「正在离开本机」，那句话是假的。
 * 形状（云）与录音圆点不同，色觉障碍用户也分得开（PRD-04-060）。
 *
 * 文案单一事实源在 `recording` 命名空间（三扇窗共用同一句，各写一份必然漂）。
 */
export function HudUplinkDot({ name, active = true }: { name: string | null; active?: boolean }) {
  const { t } = useTranslation("recording");
  const label = active
    ? name
      ? t("aria.uplink_named", { name })
      : t("aria.uplink")
    : name
      ? t("aria.uplink_idle_named", { name })
      : t("aria.uplink_idle");
  return (
    <span
      className="luna-hud-uplink-dot"
      role="img"
      data-active={active}
      aria-label={label}
      title={label}
    >
      <CloudUploadIcon size={12} />
    </span>
  );
}
