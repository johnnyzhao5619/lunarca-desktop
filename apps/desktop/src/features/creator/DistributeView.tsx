/**
 * 「分发」页签（ADR-165 ⑧，PRD-19-024 / 025）：一期视频发出去以后，再改写成别的形式。
 *
 * - **改写**：博客文章、邮件简报、推文串，从脚本或某一篇成片转写改写；
 * - **短视频片段**：从成片转写里挑出几段，给入点、出点与短视频脚本（不剪视频）。
 *
 * 每一份都由 Rust 写成项目文件夹里的一篇笔记、开头链回项目主页（`creator_distribute`），生成完
 * 直接在项目里打开它；已有的按名字列出（`creator_distribution`）。
 *
 * **按目标渠道**（ADR-166 ④ S3）：每一行是 Rust 给的一个目标——项目里一个文章 / 帖子 / 简报 /
 * 短视频渠道，或某种形态的通用一份；已有的那份带着按渠道上限重查的结果。这里只发命令、渲染
 * （ADR-003）。
 */

import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import {
  commands,
  type DistributionDto,
  type DistributionIssueDto,
  type DistributionSourceDto,
  type DistributionTargetDto,
  type PublishSourceDto,
} from "../../ipc/bindings";
import { useVaultReload } from "../../hooks/use-vault-reload";
import { noteStem } from "../../lib/note-path";
import { vaultErrorText } from "../../lib/vault-error";
import { PageLoading } from "./CreatorPage";
import { ExtraRequestField, extraRequest } from "./ExtraRequestField";
import { channelNameById, useChannels } from "./use-channels";

/** 一个目标在页面上的身份（忙碌、提示、React key 都用它）。 */
function targetKey(target: DistributionTargetDto): string {
  return `${target.kind}:${target.channel ?? ""}`;
}

/** 改写来源在单选组里的 id：`script` 或成片的路径。 */
const SCRIPT_SOURCE = "script";

export interface DistributeViewProps {
  home: string;
  /** 在项目里打开一篇（生成完、或点已有的那一份）。 */
  onOpenNote: (path: string) => void;
  /** 没有成片时去「发布」页签导入。 */
  onGoToPublish: () => void;
}

export function DistributeView({ home, onOpenNote, onGoToPublish }: DistributeViewProps) {
  const { t } = useTranslation("workspace");
  const [data, setData] = useState<DistributionDto | null>(null);
  const [sources, setSources] = useState<PublishSourceDto[] | null>(null);
  const [picked, setPicked] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [notice, setNotice] = useState<{ target: string; text: string } | null>(null);
  const catalog = useChannels();
  const [extra, setExtra] = useState("");
  const requestRef = useRef(0);

  const load = () => {
    const request = ++requestRef.current;
    void Promise.all([commands.creatorDistribution(home), commands.creatorPublishSources(home)]).then(
      ([distribution, cuts]) => {
        if (request !== requestRef.current) return;
        setData(distribution.status === "ok" ? distribution.data : { script: null, targets: [], outputs: [] });
        setSources(cuts.status === "ok" ? cuts.data : []);
      },
    );
  };
  useEffect(load, [home]);
  // 生成完一份、成片转写好了、脚本被删了：重取。
  useVaultReload(load);

  if (data === null || sources === null) return <PageLoading />;

  // 默认从脚本改写；没有脚本就用最新的那篇成片。
  const options = [...(data.script ? [SCRIPT_SOURCE] : []), ...sources.map((source) => source.path)];
  const source = picked !== null && options.includes(picked) ? picked : (options[0] ?? null);
  const sourceDto = (value: string): DistributionSourceDto =>
    value === SCRIPT_SOURCE ? { kind: "script" } : { kind: "transcript", path: value };
  // 短视频只能从成片转写挑（要时间点）：选的是脚本时用最新的那篇成片。
  const cut = source !== null && source !== SCRIPT_SOURCE ? source : (sources[0]?.path ?? null);

  const generate = async (target: DistributionTargetDto, from: string) => {
    const key = targetKey(target);
    setBusy(key);
    setNotice(null);
    const reply = await commands.creatorDistribute(home, target.kind, target.channel, sourceDto(from), extraRequest(extra));
    setBusy(null);
    if (reply.status !== "ok") {
      setNotice({ target: key, text: vaultErrorText(reply.error) });
      return;
    }
    onOpenNote(reply.data);
  };

  const issueText = (issue: DistributionIssueDto) => {
    switch (issue.kind) {
      case "post":
        return t(issue.weighted ? "creator.distribute.issue.postWeighted" : "creator.distribute.issue.post", {
          post: issue.post,
          over: issue.over,
          max: issue.max,
        });
      case "title":
        return t("creator.distribute.issue.title", { over: issue.over, max: issue.max });
      case "body":
        return t("creator.distribute.issue.body", { over: issue.over, max: issue.max });
    }
  };

  const existing = (target: DistributionTargetDto) => {
    const outputs = data.outputs.filter((output) => output.kind === target.kind && output.channel === target.channel);
    return outputs.length > 0 ? (
      <ul className="luna-distribute__outputs">
        {outputs.map((output) => (
          <li key={output.path}>
            <button type="button" className="luna-distribute__output" title={output.path} onClick={() => onOpenNote(output.path)}>
              {noteStem(output.path)}
            </button>
            {output.issues.length > 0 ? (
              <ul className="luna-distribute__issues" aria-label={t("creator.distribute.issues", { title: noteStem(output.path) })}>
                {output.issues.map((issue, index) => (
                  <li key={`${issue.kind}-${index}`}>{issueText(issue)}</li>
                ))}
              </ul>
            ) : null}
          </li>
        ))}
      </ul>
    ) : null;
  };

  const row = (target: DistributionTargetDto, from: string | null) => {
    const key = targetKey(target);
    const kindName = t(`creator.distribute.kind.${target.kind}`);
    return (
      <li key={key} className="luna-distribute__row">
        <div className="luna-distribute__text">
          <span className="luna-distribute__name">
            {target.channel === null
              ? kindName
              : t("creator.distribute.target", { channel: channelNameById(target.channel, catalog, t), kind: kindName })}
          </span>
          <span className="luna-creator-page__hint">{t(`creator.distribute.about.${target.kind}`)}</span>
          {target.channel !== null && catalog.find((channel) => channel.id === target.channel)?.ai_notice ? (
            <span className="luna-creator-page__hint" role="note">
              {t(`creator.channelNotice.${target.channel}`)}
            </span>
          ) : null}
          {existing(target)}
          {notice?.target === key ? (
            <span className="luna-creator-page__hint" role="alert">
              {notice.text}
            </span>
          ) : null}
        </div>
        <button
          type="button"
          className="luna-btn luna-btn--secondary luna-btn--sm"
          disabled={from === null || busy !== null}
          onClick={() => from !== null && void generate(target, from)}
        >
          {t(busy === key ? "creator.distribute.generating" : "creator.distribute.generate")}
        </button>
      </li>
    );
  };

  const rewrites = data.targets.filter((target) => target.kind !== "shorts");
  const shorts = data.targets.filter((target) => target.kind === "shorts");

  return (
    <div className="luna-creator-page__body">
      <ExtraRequestField value={extra} onChange={setExtra} />
      <section className="luna-creator-page__section" aria-labelledby="distribute-rewrite">
        <h2 id="distribute-rewrite">{t("creator.distribute.rewrite")}</h2>
        <p className="luna-creator-page__hint">{t("creator.distribute.rewriteHint")}</p>
        {options.length === 0 ? (
          <p className="luna-creator-page__hint">{t("creator.distribute.noSource")}</p>
        ) : (
          <div className="luna-creator-page__row">
            <span className="luna-creator-page__label">{t("creator.distribute.from")}</span>
            <div className="luna-segmented-control" role="radiogroup" aria-label={t("creator.distribute.from")}>
              {options.map((value) => (
                <button
                  key={value}
                  type="button"
                  role="radio"
                  aria-checked={source === value}
                  className="luna-segmented-control__item"
                  data-active={source === value}
                  onClick={() => setPicked(value)}
                >
                  {value === SCRIPT_SOURCE ? t("creator.distribute.fromScript") : noteStem(value)}
                </button>
              ))}
            </div>
          </div>
        )}
        <ul className="luna-distribute__rows">{rewrites.map((target) => row(target, source))}</ul>
      </section>

      <section className="luna-creator-page__section" aria-labelledby="distribute-shorts">
        <h2 id="distribute-shorts">{t("creator.distribute.shorts")}</h2>
        <p className="luna-creator-page__hint">{t("creator.distribute.shortsHint")}</p>
        {cut === null ? (
          <div className="luna-creator-page__row" role="note">
            <span className="luna-creator-page__hint">{t("creator.distribute.needsCut")}</span>
            <button type="button" className="luna-btn luna-btn--secondary luna-btn--sm" onClick={onGoToPublish}>
              {t("creator.distribute.goToPublish")}
            </button>
          </div>
        ) : (
          <p className="luna-creator-page__hint">{t("creator.distribute.shortsFrom", { title: noteStem(cut) })}</p>
        )}
        <ul className="luna-distribute__rows">{shorts.map((target) => row(target, cut))}</ul>
      </section>
    </div>
  );
}
