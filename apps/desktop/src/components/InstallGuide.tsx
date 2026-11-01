import { useState } from "react";
import { useTranslation } from "react-i18next";
import { commands, type ExternalToolGuideDto } from "../ipc/bindings";
import { CopyIcon, ExternalWindowIcon } from "./ui/Icon";

/**
 * 一件外部工具的**安装引导块**——全应用唯一实现（设置的「外部工具」面板与「从链接导入」
 * 对话框共用）。
 *
 * **两条路而不是一条**。此前引导只有"一行 `brew install …`"，它同时假定了两件事：
 * 用户在 macOS，且用户装了 Homebrew。两件都不成立时（Windows、或没装 brew 的 macOS）
 * 界面上就是一条死胡同——没有任何别的出路。现在恒给：
 *
 * - **本平台的一行命令**（macOS→Homebrew / Windows→winget，由 Rust 按平台给出；
 *   该平台没有可靠的一行命令时这一半整块不出现，绝不给一条会失败的命令）；
 * - **官方安装说明页**（恒有，经系统浏览器打开）——命令那条路走不通时的唯一出路。
 *
 * 并把"装完之后怎么办"说在原地：点面板右上角的「重新检测」，不必重启应用。此前这句
 * 只在 yt-dlp 那一处有，ffmpeg 装完的用户只能自己猜。
 *
 * 组件自持复制态：两处引导块若共用调用方的一个布尔，"已复制"会出现在用户没点的那一行下面。
 */
export interface InstallGuideProps {
  /** 本平台的引导（`null` = 还没取回来 → 什么都不渲染，不先摆一个空壳）。 */
  guide: ExternalToolGuideDto | null;
}

export function InstallGuide({ guide }: InstallGuideProps) {
  const { t } = useTranslation("settings");
  const [copied, setCopied] = useState(false);

  if (guide === null) {
    return null;
  }

  async function copy(command: string) {
    const result = await commands.popupCopy(command);
    if (result.status === "ok") {
      setCopied(true);
      window.setTimeout(() => setCopied(false), 2000);
    }
  }

  return (
    <div className="luna-install-guide">
      {guide.command !== null && guide.manager !== null && (
        <>
          <p className="luna-install-guide__label">
            {t("tools.install_with", { manager: guide.manager })}
          </p>
          <div className="luna-install-guide__command">
            <code>{guide.command}</code>
            <button
              type="button"
              className="luna-install-guide__copy"
              aria-label={t("tools.copy_command")}
              title={t("tools.copy_command")}
              onClick={() => void copy(guide.command as string)}
            >
              <CopyIcon size={14} />
            </button>
          </div>
        </>
      )}
      <div className="luna-install-guide__foot">
        {/* 恒在：包管理器不在场时它是唯一的出路，在场时它是"命令报错了怎么办"的下一步。 */}
        <button
          type="button"
          className="luna-install-guide__docs"
          onClick={() => void commands.openExternalUrl(guide.docs_url)}
        >
          <ExternalWindowIcon size={13} />
          <span>{t("tools.open_docs")}</span>
        </button>
        <span className="luna-install-guide__hint">
          {copied ? t("tools.copied") : t("tools.after_install")}
        </span>
      </div>
    </div>
  );
}
