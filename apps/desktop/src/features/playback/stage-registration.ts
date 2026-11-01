/**
 * 画面槽位的**归属**判定（ADR-118 的常驻画面层贴在谁的矩形上）。
 *
 * 登记是「后调用者胜」的裸 setter 时有一条竞态：ref 回调的 `null` **不携带身份**，
 * 而分屏 / 剧场之间切布局时 React 可以先挂新槽位、再卸旧槽位——那一次迟到的 `null`
 * 属于**旧**槽位，照单全收就把活着的那个抹掉了。后果不是"画面错位"而是**宿主从此
 * 以为自己不在回放视图**：既不给后台悬浮小窗，也不给「正在播放」条，屏幕上看起来
 * 就是"切走之后什么都没了"。
 *
 * 规则一句话：**只有当前持有者能注销自己**。判定放在纯函数里而不是散在 setState 回调
 * 里，是因为这条规则只有三种输入组合，而它们全都只在真机的布局切换时序里才出现——
 * headless 钉住它，比指望下次再撞见一次要可靠。
 */

/** 谁持有画面槽位，以及那个元素。`owner === null` ⇔ `element === null`。 */
export interface StageRegistration {
  owner: string | null;
  element: HTMLElement | null;
}

export const NO_STAGE: StageRegistration = { owner: null, element: null };

/**
 * 施加一次登记 / 注销。
 *
 * - 给元素：无条件接管（新槽位挂上来就是它了，旧持有者的注销随后会被下面那条挡掉）。
 * - 给 `null` 且自己正是持有者：真注销。
 * - 给 `null` 但持有者已经换人：**忽略**——这是旧槽位迟到的卸载回调。
 */
export function applyStageRegistration(
  current: StageRegistration,
  stageId: string,
  element: HTMLElement | null,
): StageRegistration {
  if (element !== null) {
    return { owner: stageId, element };
  }
  if (current.owner !== stageId) {
    return current;
  }
  return NO_STAGE;
}
