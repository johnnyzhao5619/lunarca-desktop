/**
 * 旧主题批量升级（ADR-134 / TDD-06 §3.9.5「选择范围 → 影响预览 → 快照/备份 → 唯一新格式」）。
 *
 * **前端零业务**：哪些页是旧格式由 Rust `topic_upgrade_candidates` 列出；每页能不能升级、
 * 升级后范围与未核验行数由 `preview_topic_upgrade` 逐页判定；确认把预览时的前后指纹原样
 * 带回，写前备份与条件写都在 Rust。逐页回执、逐页可撤销，不承诺整批原子。
 */

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  commands,
  type TopicUpgradeCandidatesDto,
  type TopicUpgradePreviewDto,
  type TopicUpgradeResultDto,
} from "../../ipc/bindings";
import { useDialogBehavior } from "../../hooks/use-dialog-behavior";
import { IconButton } from "../../components/ui/IconButton";
import { CloseIcon } from "@lunarca/editor-core/Icon";
import { noteStem } from "../../lib/note-path";

type ReadyPreview = Extract<TopicUpgradePreviewDto, { status: "ready" }>;

type Stage =
  | { kind: "loading" }
  | { kind: "select"; candidates: TopicUpgradeCandidatesDto; chosen: ReadonlySet<string> }
  | { kind: "preview"; previews: TopicUpgradePreviewDto[] }
  | { kind: "result"; results: TopicUpgradeResultDto[]; restored: ReadonlySet<string> }
  | { kind: "failed" };

export interface TopicUpgradeDialogProps {
  onClose: () => void;
}

export function TopicUpgradeDialog({ onClose }: TopicUpgradeDialogProps) {
  const { t } = useTranslation("workspace");
  const [stage, setStage] = useState<Stage>({ kind: "loading" });
  const dialogRef = useDialogBehavior<HTMLDivElement>({ onClose, enabled: true });

  useEffect(() => {
    void commands.topicUpgradeCandidates().then((result) => {
      setStage(
        result.status === "ok"
          ? {
              kind: "select",
              candidates: result.data,
              chosen: new Set(result.data.pages.map((page) => page.path)),
            }
          : { kind: "failed" },
      );
    });
  }, []);

  const preview = (paths: string[]) => {
    setStage({ kind: "loading" });
    void commands.previewTopicUpgrade(paths).then((result) => {
      setStage(result.status === "ok" ? { kind: "preview", previews: result.data } : { kind: "failed" });
    });
  };

  const apply = (ready: ReadyPreview[]) => {
    setStage({ kind: "loading" });
    void commands
      .applyTopicUpgrade(
        ready.map((item) => ({
          path: item.path,
          before_hash: item.before_hash,
          after_hash: item.after_hash,
        })),
      )
      .then((result) => {
        setStage(
          result.status === "ok"
            ? { kind: "result", results: result.data, restored: new Set() }
            : { kind: "failed" },
        );
      });
  };

  const undo = (backupId: string, path: string) => {
    void commands.restoreTopicUpgrade(backupId).then((result) => {
      if (result.status === "ok" && result.data.status === "restored") {
        setStage((current) =>
          current.kind === "result"
            ? { ...current, restored: new Set([...current.restored, path]) }
            : current,
        );
      }
    });
  };

  return (
    <div className="luna-overlay" role="presentation" onClick={onClose}>
      <div
        ref={dialogRef}
        className="luna-dialog luna-topic-upgrade"
        role="dialog"
        aria-modal="true"
        aria-label={t("topicUpgrade.title")}
        onClick={(event) => event.stopPropagation()}
      >
        <header className="luna-topic-upgrade__header">
          <h3>{t("topicUpgrade.title")}</h3>
          <IconButton label={t("topicEvidence.close")} icon={<CloseIcon size={14} />} onClick={onClose} />
        </header>
        <StageBody stage={stage} setStage={setStage} onPreview={preview} onApply={apply} onUndo={undo} onClose={onClose} />
      </div>
    </div>
  );
}

function StageBody({
  stage,
  setStage,
  onPreview,
  onApply,
  onUndo,
  onClose,
}: {
  stage: Stage;
  setStage: (stage: Stage) => void;
  onPreview: (paths: string[]) => void;
  onApply: (ready: ReadyPreview[]) => void;
  onUndo: (backupId: string, path: string) => void;
  onClose: () => void;
}) {
  const { t } = useTranslation("workspace");
  switch (stage.kind) {
    case "loading":
      return (
        <p className="luna-empty" role="status">
          {t("common:status.loading")}
        </p>
      );
    case "failed":
      return (
        <p className="luna-topic-upgrade__hint" role="alert">
          {t("topicUpgrade.failed")}
        </p>
      );
    case "select": {
      const { candidates, chosen } = stage;
      if (candidates.pages.length === 0) {
        return <p className="luna-topic-upgrade__hint">{t("topicUpgrade.none")}</p>;
      }
      const toggle = (path: string) => {
        const next = new Set(chosen);
        if (!next.delete(path)) next.add(path);
        setStage({ ...stage, chosen: next });
      };
      return (
        <>
          <p className="luna-topic-upgrade__hint">{t("topicUpgrade.selectHint")}</p>
          <ul className="luna-topic-upgrade__list">
            {candidates.pages.map((page) => (
              <li key={page.path}>
                <label className="luna-topic-upgrade__choice">
                  <input
                    type="checkbox"
                    checked={chosen.has(page.path)}
                    onChange={() => toggle(page.path)}
                  />
                  <span>{page.title}</span>
                </label>
              </li>
            ))}
          </ul>
          {candidates.omitted > 0 && (
            <p className="luna-topic-upgrade__hint">
              {t("topicUpgrade.omitted", { count: candidates.omitted })}
            </p>
          )}
          <div className="luna-dialog__actions">
            <button type="button" className="luna-btn luna-btn--secondary" onClick={onClose}>
              {t("topicEvidence.close")}
            </button>
            <button
              type="button"
              className="luna-btn luna-btn--primary"
              disabled={chosen.size === 0}
              onClick={() => onPreview([...chosen])}
            >
              {t("topicUpgrade.preview", { count: chosen.size })}
            </button>
          </div>
        </>
      );
    }
    case "preview": {
      const ready = stage.previews.filter(
        (item): item is ReadyPreview => item.status === "ready",
      );
      return (
        <>
          <ul className="luna-topic-upgrade__list">
            {stage.previews.map((item) => (
              <li key={item.path} className="luna-topic-upgrade__item" data-status={item.status}>
                <span className="luna-topic-upgrade__page">{noteStem(item.path)}</span>
                {item.status === "ready" ? (
                  <>
                    <span className="luna-topic-upgrade__detail">
                      {t("topicEvidence.upgradeImpact", {
                        scope: item.scope_title,
                        count: item.unverified_lines,
                      })}
                    </span>
                    {item.user_edited && (
                      <span className="luna-topic-upgrade__detail">
                        {t("topicEvidence.upgradeUserEdited")}
                      </span>
                    )}
                  </>
                ) : (
                  <span className="luna-topic-upgrade__detail">
                    {t(`topicEvidence.upgradeRejection.${item.code}`)}
                  </span>
                )}
              </li>
            ))}
          </ul>
          <div className="luna-dialog__actions">
            <button type="button" className="luna-btn luna-btn--secondary" onClick={onClose}>
              {t("topicEvidence.close")}
            </button>
            <button
              type="button"
              className="luna-btn luna-btn--primary"
              disabled={ready.length === 0}
              onClick={() => onApply(ready)}
            >
              {t("topicUpgrade.confirm", { count: ready.length })}
            </button>
          </div>
        </>
      );
    }
    case "result":
      return (
        <>
          <ul className="luna-topic-upgrade__list">
            {stage.results.map((item) => (
              <li key={item.path} className="luna-topic-upgrade__item" data-status={item.status}>
                <span className="luna-topic-upgrade__page">{noteStem(item.path)}</span>
                {item.status === "upgraded" ? (
                  stage.restored.has(item.path) ? (
                    <span className="luna-topic-upgrade__detail">
                      {t("topicEvidence.upgradeRestored")}
                    </span>
                  ) : (
                    <>
                      <span className="luna-topic-upgrade__detail">
                        {t(
                          item.maintenance === "ready"
                            ? "topicEvidence.upgraded"
                            : "topicEvidence.upgradedReview",
                        )}
                      </span>
                      <button
                        type="button"
                        className="luna-btn luna-btn--secondary luna-btn--sm"
                        onClick={() => onUndo(item.backup_id, item.path)}
                      >
                        {t("topicEvidence.upgradeUndo")}
                      </button>
                    </>
                  )
                ) : (
                  <span className="luna-topic-upgrade__detail">
                    {t(`topicEvidence.upgradeRejection.${item.code}`)}
                  </span>
                )}
              </li>
            ))}
          </ul>
          <div className="luna-dialog__actions">
            <button type="button" className="luna-btn luna-btn--primary" onClick={onClose}>
              {t("topicEvidence.close")}
            </button>
          </div>
        </>
      );
  }
}
