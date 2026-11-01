import { useCallback, useEffect, useRef, useState } from "react";
import { commands, type UrlProbeDto } from "../../ipc/bindings";
import { useFetchToolchain, type FetchToolchain } from "../../hooks/use-fetch-toolchain";
import { vaultErrorText } from "../../lib/vault-error";

/** 抓取形态的默认值来源（设置是唯一事实源，PRD-09；与 `useVideoPrefs` 同一 pattern）。 */
const MEDIA_SETTING_KEY = "transcribe.fetch_media";

/**
 * 下载形态三档（与 Rust `FetchMediaKind` 同构）。
 *
 * 刻意是**一个维度三个档**而不是"下载内容 × 是否保留"两组开关：那样会产出
 * 「不保留 + 视频」这种没有意义的组合（既然转写完就删，下音频最快）。
 */
export const FETCH_MEDIA_CHOICES = ["audio", "video", "link_only"] as const;
export type FetchMediaChoice = (typeof FETCH_MEDIA_CHOICES)[number];

/**
 * 「从链接导入」控制器（PRD-05-006）。
 *
 * ADR-003 前端零业务：URL 校验、站点提取、播放列表识别、直播判定、入队与顺序处理全在 Rust
 * （`probe_media_url` / `enqueue_media_url`）；这里只持**对话框的三态**与用户勾选，
 * 并把错误码原样交给渲染层去本地化。
 *
 * 三态刻意分明：`idle`（等一个地址）→ `probing`（探测中）→ 拿到 `probe`（确认卡）。
 * 中间态可被取消（关掉对话框），但**探测本身不可中断**（它是一次短命子进程，中断的收益
 * 抵不上一套取消通道的复杂度）——故用 `activeProbe` 序号丢弃过期结果，避免"先发后到"覆盖。
 */
export type UrlImportPhase = "idle" | "probing" | "confirm";

export interface UrlImport {
  phase: UrlImportPhase;
  /** 抓取工具链：状态、安装计划与安装（与「外部工具」面板同一实现）。 */
  toolchain: FetchToolchain;
  /** 探测结果（`confirm` 阶段必在场）。 */
  probe: UrlProbeDto | null;
  /** 上次探测 / 入队失败的错误码（渲染层经 `fetchErrorKey` 本地化）。 */
  error: string | null;
  /** 勾选的条目 url 集合（单条媒体默认选中；播放列表默认全选可抓条目）。 */
  selected: Set<string>;
  /** 抓取形态（初值取设置 `transcribe.fetch_media`，可在确认卡上就地改）。 */
  media: FetchMediaChoice;
  /** 入队进行中（按钮禁用）。 */
  submitting: boolean;
  /** 探测一条地址。 */
  probeUrl: (url: string) => Promise<void>;
  setMedia: (media: FetchMediaChoice) => void;
  toggle: (url: string) => void;
  toggleAll: () => void;
  /** 入队勾选项；返回入队件数（0 = 失败，错误经 `error` 回流）。 */
  submit: () => Promise<number>;
  /** 回到 `idle`（改地址重来）。 */
  reset: () => void;
}

/**
 * @param home 从创作项目「添加来源 ▸ 视频链接」来时是项目主页：转写稿建进项目的素材文件夹并链进
 *   研究笔记（ADR-167 ④）；转写页里为 `null`。
 */
export function useUrlImport(home: string | null = null): UrlImport {
  const toolchain = useFetchToolchain();
  const [phase, setPhase] = useState<UrlImportPhase>("idle");
  const [probe, setProbe] = useState<UrlProbeDto | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [media, setMedia] = useState<FetchMediaChoice>("audio");
  const [submitting, setSubmitting] = useState(false);
  /** 探测序号：只认最后一次发起的结果（用户改了地址再点，旧结果不得覆盖新结果）。 */
  const probeSeq = useRef(0);

  // 形态初值取设置（`transcribe.fetch_media`）——设置只提供"下次的默认"，用户在确认卡上
  // 就地改的选择随任务走、不回写设置（那会让"这次破例要视频"变成永久默认）。
  useEffect(() => {
    void commands.listSettings().then((result) => {
      if (result.status !== "ok") {
        return;
      }
      const value = result.data.find((spec) => spec.key === MEDIA_SETTING_KEY)?.current_value;
      if (value?.kind === "str") {
        const choice = FETCH_MEDIA_CHOICES.find((c) => c === value.value);
        if (choice) {
          setMedia(choice);
        }
      }
    });
  }, []);

  const probeUrl = useCallback(async (url: string) => {
    const seq = ++probeSeq.current;
    setPhase("probing");
    setError(null);
    setProbe(null);
    const result = await commands.probeMediaUrl(url);
    if (seq !== probeSeq.current) {
      return; // 过期结果丢弃
    }
    if (result.status === "error") {
      setError(vaultErrorText(result.error));
      setPhase("idle");
      return;
    }
    setProbe(result.data);
    // 默认全选**可抓取**条目：直播不可抓（后端已标记），预选它只会让用户在点开始时才知道。
    setSelected(
      new Set(result.data.entries.filter((entry) => !entry.is_live).map((entry) => entry.url)),
    );
    setPhase("confirm");
  }, []);

  const toggle = useCallback((url: string) => {
    setSelected((current) => {
      const next = new Set(current);
      if (next.has(url)) {
        next.delete(url);
      } else {
        next.add(url);
      }
      return next;
    });
  }, []);

  const toggleAll = useCallback(() => {
    setSelected((current) => {
      const selectable = (probe?.entries ?? []).filter((entry) => !entry.is_live);
      // 已全选 → 清空；否则补齐到全选（"全选"在部分选中时应该是"选满"，不是"清空"）。
      if (current.size >= selectable.length) {
        return new Set();
      }
      return new Set(selectable.map((entry) => entry.url));
    });
  }, [probe]);

  const submit = useCallback(async () => {
    if (!probe || submitting) {
      return 0;
    }
    const items = probe.entries
      .filter((entry) => selected.has(entry.url) && !entry.is_live)
      // filesize 原样回传：Rust 用它做**入队前的磁盘预检**（PRD-05 §5）——放不下就不该
      // 先花掉整份带宽再失败。站点未给体积时为 null，Rust 侧不猜也不阻断。
      .map((entry) => ({ url: entry.url, title: entry.title, filesize: entry.filesize }));
    if (items.length === 0) {
      return 0;
    }
    setSubmitting(true);
    setError(null);
    try {
      const result = await commands.enqueueMediaUrl(items, media, home);
      if (result.status === "error") {
        setError(vaultErrorText(result.error));
        return 0;
      }
      return result.data;
    } finally {
      setSubmitting(false);
    }
  }, [probe, selected, media, submitting, home]);

  const reset = useCallback(() => {
    probeSeq.current += 1; // 作废在飞的探测
    setPhase("idle");
    setProbe(null);
    setError(null);
    setSelected(new Set());
  }, []);

  return {
    phase,
    toolchain,
    probe,
    error,
    selected,
    media,
    submitting,
    probeUrl,
    setMedia,
    toggle,
    toggleAll,
    submit,
    reset,
  };
}
