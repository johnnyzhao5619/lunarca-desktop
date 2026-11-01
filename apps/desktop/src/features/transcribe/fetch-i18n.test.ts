import { describe, expect, it } from "vitest";
import zh from "../../i18n/locales/zh-CN/transcribe.json";
import en from "../../i18n/locales/en-US/transcribe.json";
import { FETCH_MEDIA_CHOICES } from "./use-url-import";

/**
 * 「从链接导入」的**动态 i18n key** 守护。
 *
 * 为什么需要单独一条：这些 key 由变量拼出（`` t(`fetch.media.${choice}`) ``），
 * 静态扫描 `t("…")` 的正则**看不到它们**——档位新增却漏写文案时，界面上直接显示裸 key，
 * 而 typecheck / lint / 既有测试一个都不会红。本文件把"档位集合"与"文案表"对起来。
 */

type Dict = Record<string, unknown>;

/** 按点路径取文案；缺失或非字符串返回 `undefined`。 */
function lookup(bundle: Dict, path: string): string | undefined {
  const value = path.split(".").reduce<unknown>((node, key) => {
    return typeof node === "object" && node !== null ? (node as Dict)[key] : undefined;
  }, bundle);
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

describe("从链接导入的动态文案（PRD-05-006）", () => {
  it("下载形态三档的标题与说明在中英两份文案里都在场", () => {
    // 档位是 Rust `FetchMediaKind` 的镜像；新增一档而漏写文案，这条会红。
    expect(FETCH_MEDIA_CHOICES).toEqual(["audio", "video", "link_only"]);
    for (const choice of FETCH_MEDIA_CHOICES) {
      for (const bundle of [zh, en] as unknown as Dict[]) {
        expect(lookup(bundle, `fetch.media.${choice}`), `缺 fetch.media.${choice}`).toBeTruthy();
        expect(
          lookup(bundle, `fetch.media.${choice}Hint`),
          `缺 fetch.media.${choice}Hint`,
        ).toBeTruthy();
      }
    }
  });

  it("工具四态引导的标题与说明齐备（动态 key，正则扫不到）", () => {
    // 状态由 `` t(`fetch.state.${state}Title`) `` 拼出：漏一条就是界面上一行裸 key。
    // 四态必须分立——初版把后三种糊成一句"可能是权限不足或文件损坏"，于是探测超时
    // （工具其实完全可用）也被指引去重装（2026-07-27 真机反馈）。
    for (const state of ["missing", "timeout", "broken", "checking"]) {
      for (const bundle of [zh, en] as unknown as Dict[]) {
        expect(lookup(bundle, `fetch.state.${state}Title`), `缺 ${state}Title`).toBeTruthy();
        expect(lookup(bundle, `fetch.state.${state}Body`), `缺 ${state}Body`).toBeTruthy();
      }
    }
  });

  it("仅存链接的回放降级态文案齐备（说明 + 三个动作）", () => {
    // 无媒体时播放控件整块被这几句替换掉——缺任何一条界面上就是裸 key。
    for (const key of [
      "linkOnly.playbackHint",
      "linkOnly.fetchAudio",
      "linkOnly.fetchVideo",
      "linkOnly.fetching",
      "linkOnly.openSource",
    ]) {
      for (const bundle of [zh, en] as unknown as Dict[]) {
        expect(lookup(bundle, key), `缺 ${key}`).toBeTruthy();
      }
    }
  });

  it("中英两份的 fetch 段落键集合完全一致（漏译即红）", () => {
    const keysOf = (bundle: Dict, section: string): string[] => {
      const node = bundle[section];
      return typeof node === "object" && node !== null ? Object.keys(node as Dict).sort() : [];
    };
    for (const section of ["fetch", "fetchError", "linkOnly"]) {
      expect(keysOf(zh as unknown as Dict, section), `${section} 中英键集不一致`).toEqual(
        keysOf(en as unknown as Dict, section),
      );
    }
  });
});
