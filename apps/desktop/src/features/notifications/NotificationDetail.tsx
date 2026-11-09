// 通知详情预览（PRD-07 通知中心）：历史列表只显示标题，正文/参数/元数据看不到；
// 本模态在点击某条通知时展示其**完整内容 + 元数据 + 深链动作**，让通知中心不再是
// 「看完标题即止」的死胡同。业务无副作用，仅渲染 + 转发深链导航。

import { useTranslation } from "react-i18next";
import { useDialogBehavior } from "../../hooks/use-dialog-behavior";
import type { NotificationHistoryDto } from "../../ipc/bindings";
import { resolveNotificationBaseKey } from "./notification-i18n";
import {
  formatNotificationTime,
  visibleDetailFields,
  type DetailMetaField,
} from "./notification-detail";
import { CloseIcon } from "@lunarca/editor-core/Icon";
import { HelpDetails } from "../../components/ui/HelpDetails";

interface NotificationDetailProps {
  item: NotificationHistoryDto;
  onClose: () => void;
  /** 深链导航（跳设置/权限/模型等修复入口）。 */
  onNavigate: (target: string) => void;
}

export function NotificationDetail({ item, onClose, onNavigate }: NotificationDetailProps) {
  const { t, i18n } = useTranslation("notifications");

  // 模态行为（Esc / 焦点圈定 / 焦点归还）统一走共享 hook：此处此前只有一份手写的
  // Esc 监听，没有焦点圈定也没有焦点归还（十二张卡各写各的，能力参差）。
  const dialogRef = useDialogBehavior<HTMLDivElement>({ onClose });

  const baseKey = resolveNotificationBaseKey(item.i18n_key, i18n);
  const title = t(`${baseKey}.title`, item.params);
  const body = t(`${baseKey}.body`, item.params);
  const actionLabel = i18n.exists(`${baseKey}.action`) ? t(`${baseKey}.action`) : t("action.open");
  const classLabel = t(`history.class.${item.class}`, { defaultValue: item.class });
  const severityLabel = t(`history.severity.${item.severity}`, {
    defaultValue: item.severity,
  });

  const fieldValue: Record<DetailMetaField, string> = {
    channels: item.channels
      .map((channel) => t(`detail.channel.${channel}`, { defaultValue: channel }))
      .join(t("detail.channelSeparator")),
    time: formatNotificationTime(item.created_at_ms, i18n.language),
  };
  const metadata = visibleDetailFields(item).map((field) => ({
    label: t(`detail.${field}`),
    value: fieldValue[field],
  }));

  return (
    <div className="luna-overlay" role="presentation" onClick={onClose}>
      <div
        ref={dialogRef}
        className="luna-dialog luna-notification-detail"
        role="dialog"
        aria-modal="true"
        aria-label={title}
        data-severity={item.severity}
        onClick={(event) => event.stopPropagation()}
      >
        <header className="luna-notification-detail__header">
          <div className="luna-notification-detail__badges">
            <span className="luna-notification-detail__severity" data-severity={item.severity}>
              {severityLabel}
            </span>
            <span className="luna-notification-detail__class">{classLabel}</span>
          </div>
          <button
            type="button"
            className="luna-notification-detail__close"
            aria-label={t("detail.close")}
            onClick={onClose}
          >
            <CloseIcon size={16} />
          </button>
        </header>

        <h2 className="luna-notification-detail__title">{title}</h2>
        <p className="luna-notification-detail__body">{body}</p>
        {/* 诊断（D3，2026-10-08）：版本、缺项与管理员该做的事。哪些种类有诊断由 Rust 声明，
            与正文共用同一份脱敏参数；正文只说用户影响，这里按需展开。 */}
        {item.diagnostic_i18n_key ? (
          <HelpDetails summary={t("detail.diagnostic")}>
            <p>{t(item.diagnostic_i18n_key, item.params)}</p>
          </HelpDetails>
        ) : null}

        {/* 两格都缺席时整张表不渲染——一个空的 `<dl>` 只会在正文与动作之间留一道莫名的缝。 */}
        {metadata.length > 0 && (
          <dl className="luna-notification-detail__meta">
            {metadata.map((row) => (
              <div key={row.label} className="luna-notification-detail__meta-row">
                <dt>{row.label}</dt>
                <dd>{row.value}</dd>
              </div>
            ))}
          </dl>
        )}

        <footer className="luna-notification-detail__actions">
          {item.target ? (
            <button
              type="button"
              className="luna-btn luna-btn--primary"
              onClick={() => {
                onNavigate(item.target as string);
                onClose();
              }}
            >
              {actionLabel}
            </button>
          ) : null}
          <button type="button" className="luna-btn luna-btn--secondary" onClick={onClose}>
            {t("detail.close")}
          </button>
        </footer>
      </div>
    </div>
  );
}
