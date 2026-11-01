/**
 * 起播意图的判据（PRD-04-064/066）——**纯函数，可无头测试**。
 *
 * 它被提出来，是因为写在 effect 里的那一版错得很安静：第一版用的是一个只增不减的
 * nonce（`autoplayNonce !== 0` 即起播），而 effect 的依赖里有 `loading`。于是用户点过一次
 * 「在此播放」之后，此后**每打开一篇新的录音或转写**，`loading` 落下都会再次满足条件——
 * "点开看看"变成了"点开就放"（真机 2026-09-08 上报「切换到转写和录音时自动播放」）。
 *
 * 那种错误在类型上、lint 上、渲染上都看不出来，只有把规则单独写成一句话才看得见：
 * **起播是一次性的意图，且必须指名道姓属于哪一篇。**
 */

/** 一次待执行的起播意图。 */
export interface PlayIntent {
  /** 要起播的那一篇（Vault 相对路径）。 */
  note: string;
  /** 同一篇被再次请求起播时自增——它只用来触发重新判定，不参与判据。 */
  nonce: number;
  /**
   * 起播前先跳到这一毫秒（时间戳行的「从这一句开始听」）。
   *
   * 它必须随意图一起走，而不是在 `open()` 之后立刻 seek：换篇时 `open()` 返回那一刻
   * 媒体还没换源，seek 打在旧媒体上，随后换源把 `currentTime` 归零——新篇从头起播，
   * 点的那一句丢了。
   */
  seekMs?: number;
}

/** `open()` 的选项。 */
export interface OpenOptions {
  /** 装好即起播。 */
  play?: boolean;
  /** 起播前先跳到这一毫秒；只与 `play` 同用。 */
  seekMs?: number;
}

/**
 * `open(note, options)` 之后待执行的意图应当是什么。
 *
 * - 带 `play`：新意图指名这一篇，`nonce` 自增（同一篇再点一次也要重新起播）；
 * - 不带 `play` 且换到**别篇**：旧意图作废。不作废的话，点过「在此播放」后飞快切到
 *   别篇（或那一篇根本没有本地媒体），那枚意图会一直悬着，下次普通点开原篇就自动起播；
 * - 不带 `play` 且还是同一篇：保留（回放视图挂载时会对同一篇再 `open()` 一次）。
 */
export function intentOnOpen(
  prev: PlayIntent | null,
  note: string,
  options?: OpenOptions,
): PlayIntent | null {
  if (options?.play) {
    return { note, nonce: (prev?.nonce ?? 0) + 1, seekMs: options.seekMs };
  }
  return prev !== null && prev.note === note ? prev : null;
}

export interface AutoplayInput {
  /** 待执行的意图；`null` = 没有人要求起播。 */
  intent: PlayIntent | null;
  /**
   * 宿主**装载完成**的是哪一篇；`null` = 空载或还在装。
   *
   * 必须是"装完的"而不是"要装的"（`noteRel`）：换篇那一帧后者已经变了而装载还没开始，
   * 拿它判会把意图消费在旧媒体上（真机 2026-09-08「在此播放要再点一次」）。
   */
  loadedNote: string | null;
  /** 聚合包还在路上。 */
  loading: boolean;
  /** 这一篇有本地媒体可播（「仅存链接」的转写没有）。 */
  hasMedia: boolean;
}

/**
 * 此刻该不该起播。
 *
 * 四条缺一不可：
 * ① 真的有人要求过（意图非空）——**没有意图就绝不自作主张**，这是那次回归的正解；
 * ② 装的就是被要求的那一篇——用户点完又飞快切了一篇时，不该把别人那篇放出来；
 * ③ 装载完成；④ 确实有媒体。
 */
export function shouldAutoplay(input: AutoplayInput): boolean {
  const { intent, loadedNote, loading, hasMedia } = input;
  if (intent === null) return false;
  if (loadedNote !== intent.note) return false;
  if (loading) return false;
  return hasMedia;
}
