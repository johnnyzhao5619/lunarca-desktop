/**
 * 启用「创作」后的入门（ADR-165 ⑥ / PRD-19-036）：三步，都可以跳过，之后都能改。
 *
 * ① 你常发哪些渠道（内置渠道多选，至少一个）：存成「我的渠道」、都作新项目默认
 *    （`creator_set_channel_favorites`，ADR-166 ⑦）；之后在发布页「管理渠道…」里细调；
 * ② 创作文件放在库里哪个文件夹（`vault.creator_dir`，去设置里改）；
 * ③ 从示例项目开始（`creator_create_example_project`：每个阶段都填好一点内容），或新建第一个项目。
 *
 * 存渠道与建示例都走 Rust，这里只发命令（ADR-003）。
 */

import { useState } from "react";
import { useTranslation } from "react-i18next";

import { commands } from "../../ipc/bindings";
import { useStringSetting } from "../../hooks/use-bool-setting";
import { goTo } from "../../lib/navigate";
import { vaultErrorText } from "../../lib/vault-error";
import { creatorRoute } from "../workspace/workspace-routes";
import { channelName, saveFavorites, useChannelFavorites, useChannels } from "./use-channels";

export function CreatorSetup({ onDone }: { onDone: () => void }) {
  const { t } = useTranslation("workspace");
  const builtin = useChannels().filter((channel) => channel.builtin);
  const favorites = useChannelFavorites();
  const on = new Set(favorites.map((favorite) => favorite.id));
  const folder = useStringSetting("vault.creator_dir", "Creator");
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);

  const toggle = async (id: string) => {
    // 至少留一个（Rust 同样兜底）；新勾的都作新项目默认。
    const next = on.has(id)
      ? favorites.filter((favorite) => favorite.id !== id)
      : [...favorites, { id, default: true }];
    if (next.length === 0) return;
    setNotice(null);
    const error = await saveFavorites(next);
    if (error) setNotice(vaultErrorText(error));
  };

  const openExample = async () => {
    setBusy(true);
    setNotice(null);
    const reply = await commands.creatorCreateExampleProject();
    setBusy(false);
    if (reply.status !== "ok") {
      setNotice(vaultErrorText(reply.error));
      return;
    }
    onDone();
    goTo(creatorRoute({ view: "project", home: reply.data.path, tab: "overview" }));
  };

  return (
    <section className="luna-creator-setup" aria-labelledby="creator-setup-title">
      <header className="luna-creator-setup__header">
        <h2 id="creator-setup-title">{t("creator.setup.title")}</h2>
        <p className="luna-creator-page__hint">{t("creator.setup.lede")}</p>
      </header>
      <ol className="luna-creator-setup__steps">
        <li className="luna-creator-setup__step">
          <span className="luna-creator-setup__name">{t("creator.setup.platforms")}</span>
          <div className="luna-creator-page__choices" role="group" aria-label={t("creator.setup.platforms")}>
            {builtin.map((channel) => (
              <button
                key={channel.id}
                type="button"
                className="luna-creator-page__choice"
                aria-pressed={on.has(channel.id)}
                onClick={() => void toggle(channel.id)}
              >
                {channelName(channel, t)}
              </button>
            ))}
          </div>
        </li>
        <li className="luna-creator-setup__step">
          <span className="luna-creator-setup__name">{t("creator.setup.folder")}</span>
          <span className="luna-creator-page__hint">{t("creator.setup.folderValue", { folder })}</span>
          <button
            type="button"
            className="luna-btn luna-btn--secondary luna-btn--sm"
            onClick={() => goTo("settings://notes/vault.creator_dir")}
          >
            {t("creator.setup.changeFolder")}
          </button>
        </li>
        <li className="luna-creator-setup__step">
          <span className="luna-creator-setup__name">{t("creator.setup.start")}</span>
          <button type="button" className="luna-btn luna-btn--primary luna-btn--sm" disabled={busy} onClick={() => void openExample()}>
            {t("creator.setup.example")}
          </button>
          <button
            type="button"
            className="luna-btn luna-btn--secondary luna-btn--sm"
            onClick={() => {
              onDone();
              goTo("creator://");
            }}
          >
            {t("creator.setup.blank")}
          </button>
        </li>
      </ol>
      {notice ? (
        <p className="luna-creator-page__hint" role="alert">
          {notice}
        </p>
      ) : null}
      <div className="luna-creator-page__row">
        <button type="button" className="luna-btn luna-btn--secondary luna-btn--sm" onClick={onDone}>
          {t("creator.setup.skip")}
        </button>
      </div>
    </section>
  );
}
