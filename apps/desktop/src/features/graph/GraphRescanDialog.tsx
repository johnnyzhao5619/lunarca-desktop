/**
 * AI 关系标签全库重扫确认卡（PRD-06-035 全库重扫入口）。
 *
 * 两步流程而非「点了就跑」：预检先把代价摆出来（范围、扫多少篇、跳过多少篇、**实际在哪执行**、
 * 写入什么），数字在点开始**之前**给。执行位置是 Rust 按对话主干的落点算出来的
 * （`preflight.execution`）——此前一句固定的「生成在本地逐篇进行」在连着服务端时是假话。两档动作是一个维度的两个取值——「只补缺口」按指纹跳过已最新
 * 的笔记（默认，绝大多数场景），「全部重新生成」连已最新的一起重扫（换了模型 / 词表大改
 * 之后才需要）；不给「强制 + 只补缺口」这类说不清的组合。
 *
 * 已有一次重扫在跑时不给第二个开始按钮（Rust 侧单例判据同源），只陈述状态。
 *
 * **规则的结果可见还不够，规则本身也必须可见**（2026-07-30 真机第二轮的 D7）：此前这里
 * 只报一个"N 篇在排除范围"，而真机现场那 N 篇里有 15 篇是用户**某次误点停用了整个 `Notes`
 * 文件夹**——那一下让整个知识面静默失效（标签长不出来 → 主题页建议永不触发 → 归入链路
 * 整条够不着），而他连"是什么挡住的"都看不见，更没有入口去改。故本卡把两档**分开报**
 * （类型白名单是设计、停用文件夹是用户的设置），并把被停用的文件夹**逐条列出 + 就地取消**，
 * 取消后立即重取预检——数字与规则必须描述同一时刻的事实。
 */

import { useTranslation } from "react-i18next";
import { useDialogBehavior } from "../../hooks/use-dialog-behavior";
import { commands } from "../../ipc/bindings";
import { TagsIcon } from "../../components/ui/Icon";
import { FactRow } from "../../components/ui/FactRow";
import type { GraphController } from "./use-graph-controller";

export function GraphRescanDialog({ controller }: { controller: GraphController }) {
  const { t } = useTranslation("workspace");
  const { rescanDialog, closeRescanDialog, startRescan, openRescanDialog } = controller;
  const open = rescanDialog.kind !== "closed";

  // 模态行为（Esc / 焦点圈定 / 焦点归还）统一走共享 hook：此处此前只有一份手写的
  // Esc 监听，没有焦点圈定也没有焦点归还（十二张卡各写各的，能力参差）。
  const dialogRef = useDialogBehavior<HTMLDivElement>({
    onClose: closeRescanDialog,
    enabled: open,
  });

  if (rescanDialog.kind === "closed") {
    return null;
  }

  const renderBody = () => {
    if (rescanDialog.kind === "loading") {
      return <p className="luna-dialog__note">{t("graph.rescanPreflight")}</p>;
    }
    if (rescanDialog.kind === "error") {
      return (
        <p className="luna-field-error" role="alert">
          {rescanDialog.message}
        </p>
      );
    }
    const { preflight, startError, scope, emphasis } = rescanDialog;
    if (preflight.running) {
      return <p className="luna-dialog__note">{t("graph.rescanAlreadyRunning")}</p>;
    }
    const nothingToScan = preflight.stale === 0 && preflight.fresh === 0;
    return (
      <>
        <div className="luna-fact-list">
          <FactRow
            label={t("graph.rescanScope")}
            value={scope === null ? t("graph.rescanScopeAllValue") : scope}
          />
          <FactRow label={t("graph.rescanStale")} value={preflight.stale} />
          <FactRow label={t("graph.rescanFresh")} value={preflight.fresh} />
          {preflight.empty > 0 && <FactRow label={t("graph.rescanEmpty")} value={preflight.empty} />}
          {/* 类型白名单挡下的（资料区与速记）：这一档是设计，用户无须处置。 */}
          {preflight.excluded_type > 0 && (
            <FactRow label={t("graph.rescanExcludedType")} value={preflight.excluded_type} />
          )}
          {/* 用户自己停用的文件夹挡下的：这一档是**一个设置**，下方列出规则并给入口去改。 */}
          {preflight.excluded_folder > 0 && (
            <FactRow label={t("graph.rescanExcludedFolder")} value={preflight.excluded_folder} />
          )}
          <FactRow
            label={t("graph.rescanExecution")}
            value={t(`graph.rescanExecutionAt.${preflight.execution}`)}
          />
          <FactRow label={t("graph.rescanWrites")} value={t("graph.rescanWritesValue")} />
          <p className="luna-fact-list__note">{t("graph.rescanCost")}</p>
        </div>
        {preflight.excluded_folders.length > 0 && (
          <div className="luna-graph-rescan__exclusions">
            <p className="luna-dialog__note">{t("graph.rescanExclusionsHint")}</p>
            <ul>
              {preflight.excluded_folders.map((folder) => (
                <li key={folder}>
                  <TagsIcon size={13} />
                  <span className="luna-graph-rescan__exclusion-name">{folder}</span>
                  <button
                    type="button"
                    className="luna-btn luna-btn--secondary"
                    onClick={() => {
                      // 就地取消 → 立即重取预检：数字与规则必须描述同一时刻的事实
                      // （否则用户取消了却看到"还是 27 篇被挡住"，只会以为没生效）。
                      void commands.setGraphTagExclusion(folder, false).then((result) => {
                        if (result.status === "ok") {
                          openRescanDialog(scope);
                        }
                      });
                    }}
                  >
                    {t("graph.rescanExclusionRemove")}
                  </button>
                </li>
              ))}
            </ul>
          </div>
        )}
        {nothingToScan && <p className="luna-dialog__note">{t("graph.rescanNothing")}</p>}
        {startError && (
          <p className="luna-field-error" role="alert">
            {startError}
          </p>
        )}
        <div className="luna-dialog__actions">
          <button
            type="button"
            className="luna-btn luna-btn--secondary"
            onClick={closeRescanDialog}
          >
            {t("graph.rescanCancel")}
          </button>
          {/* 「全部重新生成」在缺口为零但存量可刷时仍可用——那正是它存在的场景。
              **主次由 `emphasis` 决定**：从「标签语言分裂」那张维护卡进来时，语言不
              一致的笔记指纹是最新的，「只补缺口」按指纹跳过、一篇都扫不到（且多半是
              禁用的）——默认主次在那个场景下恰好是反的。 */}
          <button
            type="button"
            className={`luna-btn ${emphasis === "force" ? "luna-btn--primary" : "luna-btn--secondary"}`}
            disabled={preflight.stale === 0 && preflight.fresh === 0}
            onClick={() => void startRescan(true, scope)}
          >
            {t("graph.rescanForce", { count: preflight.stale + preflight.fresh })}
          </button>
          <button
            type="button"
            className={`luna-btn ${emphasis === "force" ? "luna-btn--secondary" : "luna-btn--primary"}`}
            disabled={preflight.stale === 0}
            onClick={() => void startRescan(false, scope)}
          >
            {t("graph.rescanStart", { count: preflight.stale })}
          </button>
        </div>
      </>
    );
  };

  return (
    <div className="luna-overlay" role="presentation" onClick={closeRescanDialog}>
      <div
        ref={dialogRef}
        className="luna-dialog"
        role="dialog"
        aria-modal="true"
        aria-label={t("graph.rescanTitle")}
        onClick={(event) => event.stopPropagation()}
      >
        <header className="luna-dialog__header">
          <h3>{t("graph.rescanTitle")}</h3>
          {/* 范围陈述（PRD-06-035c）：定向重扫时明示对象，杜绝"以为在扫整库"——它是事实清单的
              第一行（预检回来之前就写在页头，盘点期间也看得见在扫哪儿）。 */}
          {rescanDialog.kind !== "ready" && (
            <p className="luna-dialog__scope">
              {rescanDialog.scope === null
                ? t("graph.rescanScopeAll")
                : t("graph.rescanScopeFolder", { folder: rescanDialog.scope })}
            </p>
          )}
          <p>{t("graph.rescanDesc")}</p>
        </header>
        {renderBody()}
      </div>
    </div>
  );
}
