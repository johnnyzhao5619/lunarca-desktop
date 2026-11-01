import { describe, expect, it } from "vitest";
import zh from "../i18n/locales/zh-CN/settings.json";
import en from "../i18n/locales/en-US/settings.json";
import { EXTERNAL_TOOLS } from "./use-install-guides";

/**
 * 外部工具清单的**跨界守护**（PRD-05-007 / PRD-09-028）。
 *
 * 后端 `luna_fetch::tool::ExternalTool::ALL` 决定「有哪几件工具」，前端这份常量是它的镜像。
 * 两边各写一份就必然漂移，而漂移的表现极不显眼：后端新增一件工具，前端的 `find` 悄悄返回
 * `undefined`，那一行的安装引导整块消失——没有报错、没有裸 key，只是"少了点东西"。
 *
 * 故这里做两件事：把清单**钉死**（改动必须是刻意的，会红在这条上），并要求每件工具在中英
 * 两份文案里都有名字与说明——工具名与 i18n key 的对应关系正是 `tools.<id>_label`。
 */

type Dict = Record<string, unknown>;

const BUNDLES = [zh, en] as unknown as Dict[];

/** 按点路径取文案；缺失或非字符串返回 `undefined`。 */
function lookup(bundle: Dict, path: string): string | undefined {
  const value = path.split(".").reduce<unknown>((node, key) => {
    return typeof node === "object" && node !== null ? (node as Dict)[key] : undefined;
  }, bundle);
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

describe("外部工具清单与安装引导", () => {
  it("工具 id 集合与后端 `ExternalTool::as_str` 逐字一致", () => {
    // 后端顺序即面板呈现顺序；改动两边都要改，故这条断言故意写死。
    expect(EXTERNAL_TOOLS).toEqual(["yt_dlp", "ffmpeg", "js_runtime"]);
  });

  it("每件工具在中英两份文案里都有名字与说明", () => {
    for (const tool of EXTERNAL_TOOLS) {
      for (const bundle of BUNDLES) {
        expect(lookup(bundle, `tools.${tool}_label`), `缺 tools.${tool}_label`).toBeTruthy();
        expect(lookup(bundle, `tools.${tool}_desc`), `缺 tools.${tool}_desc`).toBeTruthy();
      }
    }
  });

  it("安装引导块的文案齐备，且**没有任何一条写死的安装命令**", () => {
    // 引导块共用一套文案（`InstallGuide`），缺一条就是界面上一行裸 key。
    for (const key of [
      "tools.install_with",
      "tools.open_docs",
      "tools.after_install",
      "tools.copy_command",
      "tools.copied",
      "tools.recheck_all",
      "tools.more_actions",
    ]) {
      for (const bundle of BUNDLES) {
        expect(lookup(bundle, key), `缺 ${key}`).toBeTruthy();
      }
    }

    // 命令是**环境事实**不是文案：它随平台变、不随语言变。写在 i18n 里的那份曾经让
    // Windows 用户复制到一条必然失败的 `brew install …`，且没有任何门禁能发现。
    // 现在它只能来自 Rust（`luna_fetch::tool::install_guide`），故文案里一条都不许有。
    for (const bundle of BUNDLES) {
      const serialized = JSON.stringify(bundle["tools"] ?? {});
      for (const manager of ["brew install", "winget install", "apt install", "choco install"]) {
        expect(serialized, `文案里出现了写死的安装命令：${manager}`).not.toContain(manager);
      }
    }
  });
});
