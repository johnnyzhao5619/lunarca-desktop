// 系统权限说明与预授权（PRD-14-030/031）：用人话讲清用途，状态真实可查、可现在授予、
// 全部可跳过（相关功能首次使用时再引导）。查询/降级决策在 Rust check_permission。
//
// 保留这一步而不并入「第一个任务」（ADR-114 ④ ⑷）：辅助功能在 macOS 要去系统设置手动勾选，
// 提前讲清楚能降低那一趟的失败率；麦克风则在第一个任务里按需申请（Apple HIG）。
// 行序 = 下一步先用到谁：麦克风（语音输入）→ 辅助功能（弹窗翻译的双击复制 / 替换原文）→ 通知。
//
// 状态取自壳层已有的 `setup_status`（它本就下发这三项）。此前这里另查一遍并自挂 focus
// 监听，而 `useSetupStatus` 挂着同一个 focus 监听——窗口每聚焦一次就是六次系统权限查询，
// 同一件事问两遍还可能给出两个答案。去系统设置那一趟由那条订阅接住，这里不必再挂。

import { useTranslation } from "react-i18next";
import {
  commands,
  type PermissionKind,
  type PermissionStatus,
  type SetupStatusDto,
} from "../../../ipc/bindings";
import { BellIcon, KeyboardIcon, MicIcon } from "@lunarca/editor-core/Icon";

/**
 * 首启引导内预先说明的权限（PRD-14-030 表；屏幕录制为 P1 随会议录音接入，不在此列）。
 *
 * `kind` 同时钉成 `SetupStatusDto` 上那一项的**字段名**：Rust 改名或少下发一项时 tsc
 * 当场红，而不是在界面上静默退回「未询问」。
 */
const PERMISSION_ROWS = [
  { kind: "microphone", Icon: MicIcon },
  { kind: "accessibility", Icon: KeyboardIcon },
  { kind: "notification", Icon: BellIcon },
] as const satisfies ReadonlyArray<{
  kind: PermissionKind & keyof SetupStatusDto;
  Icon: typeof MicIcon;
}>;

/** 权限状态 → 徽标语气。已给是成功，被拒是错误，其余（未询问 / 查不到）是中性事实。 */
const STATUS_TONE: Record<PermissionStatus, "success" | "error" | "muted"> = {
  granted: "success",
  denied: "error",
  not_determined: "muted",
  unsupported: "muted",
};

export function PermissionsStep({
  setup,
  onRefresh,
}: {
  setup: SetupStatusDto | null;
  /** 刚授过权 → 让壳层重读装机状态（横幅与三态读的是同一份）。 */
  onRefresh: () => void;
}) {
  const { t } = useTranslation("onboarding");
  // 这个平台一项都查不到（Windows 上三项皆是）：三行「系统不提供查询」+ 三颗灰按钮
  // 读起来像应用坏了，而实情是这里没有任何需要用户做的事。收成一句陈述。
  // 状态未到时按 `not_determined` 算，故不会在加载中途误判成"无需授权"。
  const noneQueryable = PERMISSION_ROWS.every(
    ({ kind }) => (setup?.[kind] ?? "not_determined") === "unsupported",
  );

  if (noneQueryable) {
    return (
      <p className="luna-onboarding__hint">{t("step.permissions.none_required")}</p>
    );
  }

  return (
    <div className="luna-onboarding-step">
      <ul className="luna-onboarding-list">
        {PERMISSION_ROWS.map(({ kind, Icon }) => {
          const status = setup?.[kind] ?? "not_determined";
          const granted = status === "granted";
          return (
            <li key={kind} className="luna-onboarding-item">
              <span
                className="luna-onboarding-item__icon"
                data-tone={granted ? "success" : "muted"}
                aria-hidden="true"
              >
                <Icon size={18} />
              </span>
              <div className="luna-onboarding-item__text">
                <div className="luna-onboarding-permission__name">
                  <strong>{t(`step.permissions.kind.${kind}.name`)}</strong>
                  <span className="luna-onboarding__badge" data-tone={STATUS_TONE[status]}>
                    {t(`step.permissions.status.${status}`)}
                  </span>
                </div>
                <p>{t(`step.permissions.kind.${kind}.purpose`)}</p>
                {/* 「不现在给也行……」只对**还没给**的那一项成立：已授权还留着它，是在给一件
                    已经办完的事写补救说明，也让三行权限各有三段文字、这一步比别处长出一截。 */}
                {granted ? null : <p>{t(`step.permissions.kind.${kind}.later`)}</p>}
              </div>
              {/* 已授权 / 系统不提供查询时**不摆那颗灰按钮**：一颗按不动的「去授权」紧挨着
                  「已授权」，读起来像"还差点什么"。 */}
              {granted || status === "unsupported" ? null : (
                <div className="luna-onboarding-item__action">
                  <button
                    type="button"
                    className="luna-btn luna-btn--secondary"
                    onClick={() => {
                      void commands.requestPermission(kind).then(() => {
                        // 系统弹框可能同步返回（麦克风），稍候重读一次兜底。
                        window.setTimeout(onRefresh, 1000);
                      });
                    }}
                  >
                    {t("step.permissions.grant")}
                  </button>
                </div>
              )}
            </li>
          );
        })}
      </ul>
      <div className="luna-onboarding-permissions__footer">
        <p className="luna-onboarding__hint">{t("step.permissions.skip_hint")}</p>
        <button type="button" className="luna-btn luna-btn--secondary" onClick={onRefresh}>
          {t("action.recheck")}
        </button>
      </div>
    </div>
  );
}
