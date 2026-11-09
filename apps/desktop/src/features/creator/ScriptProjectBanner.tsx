/**
 * 「笔记」空间里打开项目里的脚本时，编辑器上方的一条横幅（ADR-165 ⑦ / PRD-19-038）：
 * 「这是《项目名》的脚本 · 在创作中打开」。
 *
 * 写脚本的工具（时长、提词器、口述、改稿、调段序、引文）在创作空间的脚本工作区里，这里不复刻
 * 一份——只把人带过去。属于哪个项目由 Rust 判（`creator_project_of`，与引文同一判定）；不属于
 * 任何项目就不显示。宿主只在「创作」启用、这一篇是脚本时挂它。
 */

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { commands, type ProjectRefDto } from "../../ipc/bindings";
import { FilmIcon } from "@lunarca/editor-core/Icon";
import { goTo } from "../../lib/navigate";
import { creatorRoute } from "../workspace/workspace-routes";

export function ScriptProjectBanner({ path }: { path: string }) {
  const { t } = useTranslation("workspace");
  const [project, setProject] = useState<ProjectRefDto | null>(null);

  useEffect(() => {
    let cancelled = false;
    setProject(null);
    void commands.creatorProjectOf(path).then((reply) => {
      if (!cancelled && reply.status === "ok") setProject(reply.data);
    });
    return () => {
      cancelled = true;
    };
  }, [path]);

  if (!project) return null;
  return (
    <div className="luna-script-banner" role="note">
      <FilmIcon size={14} />
      <span className="luna-script-banner__text">{t("creator.script.banner", { project: project.title })}</span>
      <button
        type="button"
        className="luna-btn luna-btn--secondary luna-btn--sm"
        onClick={() => goTo(creatorRoute({ view: "project", home: project.home, tab: "script" }))}
      >
        {t("creator.script.openInCreator")}
      </button>
    </div>
  );
}
