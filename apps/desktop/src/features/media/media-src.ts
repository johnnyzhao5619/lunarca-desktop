import { convertFileSrc } from "@tauri-apps/api/core";
import { commands } from "../../ipc/bindings";

export async function vaultImageAssetSrc(notePath: string, relativePath: string) {
  const result = await commands.vaultAssetPath(notePath, relativePath);
  if (result.status === "error") {
    throw new Error(result.error);
  }
  return convertFileSrc(result.data, "asset");
}

/**
 * 直接打开的 Vault 文件（非笔记内嵌）→ asset 协议 URL（PRD-06-007 图片预览标签）。
 * 复用 note-relative 解析：以文件自身为「笔记」、其文件名为「相对附件」，解析回文件本身。
 */
export async function vaultFileAssetSrc(relativePath: string) {
  const fileName = relativePath.split("/").pop() ?? relativePath;
  return vaultImageAssetSrc(relativePath, fileName);
}

/**
 * 受管本地媒体（音频/视频）的**直链流式 URL**，供 `crossorigin="anonymous"` 的媒体元素
 * 原生 Range 播放。
 *
 * `<audio>`/`<video>` 元素无法设置 `Authorization` 头，故 per-session token 经 `?token=` 传递
 * （媒体服务同时校验 Origin/CORS，仅绑 127.0.0.1，每会话轮换；TDD-12 §4 / TDD-07 §3）。
 * **绝不整段 `fetch().blob()`**——避免大音频/视频一次性载入 Webview 触发 OOM（TDD-07 §3）；
 * 视频 seek 依赖的正是这条 Range 通道。
 */
export async function vaultMediaStreamUrl(relativePath: string): Promise<string> {
  const info = await commands.mediaServerInfo();
  const url = new URL("/stream", info.base_url);
  url.searchParams.set("path", relativePath);
  url.searchParams.set("token", info.token);
  return url.toString();
}

/**
 * 多分片虚拟拼接资源的流式 URL（`load_sync_playback` 返回的 composite_id）。
 *
 * 同一鉴权与 Range 机制（见 [vaultMediaStreamUrl]）；多分片录音对 `<audio>`
 * 呈现为单一连续 WAV，时间轴即会话绝对时钟（PRD-04 §5）。
 */
export async function vaultCompositeStreamUrl(compositeId: string): Promise<string> {
  const info = await commands.mediaServerInfo();
  const url = new URL("/stream", info.base_url);
  url.searchParams.set("composite", compositeId);
  url.searchParams.set("token", info.token);
  return url.toString();
}
