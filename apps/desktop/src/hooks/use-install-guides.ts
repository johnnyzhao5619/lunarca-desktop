import { useEffect, useState } from "react";
import { commands, type ExternalToolGuideDto } from "../ipc/bindings";

/**
 * 外部工具的稳定 id（Rust `luna_fetch::tool::ExternalTool::as_str` 的镜像）。
 *
 * 与 `FETCH_MEDIA_CHOICES` 同一条跨界口径：`luna-fetch` 保持零新增依赖（不引 specta），
 * 枚举以稳定字符串过界，故前端一侧用常量元组把取值集合钉住，由
 * `use-install-guides.test.ts` 守护"后端新增一件工具、前端却没有对应文案"。
 */
export const EXTERNAL_TOOLS = ["yt_dlp", "ffmpeg", "js_runtime"] as const;

/** 外部工具 id。 */
export type ExternalToolId = (typeof EXTERNAL_TOOLS)[number];

/**
 * 取本机平台的安装引导（**一次 IPC，之后常驻**）。
 *
 * 为什么它必须来自后端：安装命令是**环境事实**而不是文案——它随平台变、不随语言变。
 * 此前它写死在 i18n 里（`brew install …`），于是 Windows 用户复制到的是一条必然失败的命令，
 * 而界面上没有第二条路（2026-08-23 审查）。放前端就必然分裂成"语言 × 平台"份，
 * 且没有任何门禁能发现其中哪一份是错的。
 *
 * 返回 `null` 表示还没取回来——调用方据此**不渲染**引导块，而不是先渲染一个空壳。
 */
export function useInstallGuides(): ((tool: ExternalToolId) => ExternalToolGuideDto | null) | null {
  const [guides, setGuides] = useState<readonly ExternalToolGuideDto[] | null>(null);

  useEffect(() => {
    let active = true;
    void commands.externalToolGuides().then((list) => {
      if (active) {
        setGuides(list);
      }
    });
    return () => {
      active = false;
    };
  }, []);

  if (guides === null) {
    return null;
  }
  return (tool) => guides.find((guide) => guide.tool === tool) ?? null;
}
