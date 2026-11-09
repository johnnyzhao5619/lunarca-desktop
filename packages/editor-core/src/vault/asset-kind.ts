// 非 Markdown 文件类型分类（PRD-06-007，Obsidian 按扩展路由视图对标）。纯逻辑、可测。
//
// 白名单驱动：仅**已知可承接**的附件扩展返回具体 kind（图片/音视频/文档），其余（.md /
// 无扩展的文件夹 / 未知扩展）返回 null → 交由笔记编辑器/文件夹卡片处理。白名单避免把
// 「Cardian Academy v1.0.0」这类带点文件夹误判为二进制文件。

export type AssetKind = "image" | "audio" | "video" | "pdf" | "binary";

const IMAGE_EXTS = new Set([
  "png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "tiff", "tif", "avif", "ico",
]);
const AUDIO_EXTS = new Set(["mp3", "wav", "ogg", "oga", "m4a", "flac", "aac", "opus"]);
const VIDEO_EXTS = new Set(["mp4", "webm", "mov", "m4v", "mkv"]);
// 文档/归档类：无法内嵌预览，走「外部程序打开」。
const BINARY_EXTS = new Set([
  "doc", "docx", "xls", "xlsx", "ppt", "pptx", "zip", "rar", "7z", "csv", "key",
  "numbers", "pages", "epub",
]);

/** 取小写扩展名（无扩展或以点开头的隐藏文件名返回空串）。 */
export function extensionOf(path: string): string {
  const name = path.split("/").pop() ?? path;
  const dot = name.lastIndexOf(".");
  if (dot <= 0) {
    return "";
  }
  return name.slice(dot + 1).toLowerCase();
}

/**
 * 分类一个打开目标：返回附件 kind，或 null（= 笔记 / 文件夹 / 未知，走 VaultEditor）。
 * `.md` 与无扩展/未知扩展一律 null（后者含带点文件夹，交编辑器按文件夹渲染）。
 */
export function classifyAssetTarget(path: string | null): AssetKind | null {
  if (!path) {
    return null;
  }
  const ext = extensionOf(path);
  if (ext === "" || ext === "md") {
    return null;
  }
  if (IMAGE_EXTS.has(ext)) return "image";
  if (AUDIO_EXTS.has(ext)) return "audio";
  if (VIDEO_EXTS.has(ext)) return "video";
  if (ext === "pdf") return "pdf";
  if (BINARY_EXTS.has(ext)) return "binary";
  return null;
}
