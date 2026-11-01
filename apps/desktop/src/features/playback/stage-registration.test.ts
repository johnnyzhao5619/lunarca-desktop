import { describe, expect, it } from "vitest";
import { applyStageRegistration, NO_STAGE } from "./stage-registration";

/** 只关心身份，元素本身不参与判定——用两个可区分的桩即可。 */
const A = { id: "A" } as unknown as HTMLElement;
const B = { id: "B" } as unknown as HTMLElement;

describe("画面槽位的归属", () => {
  it("挂上来就接管", () => {
    expect(applyStageRegistration(NO_STAGE, "a", A)).toEqual({ owner: "a", element: A });
  });

  it("持有者自己注销才真注销", () => {
    const held = applyStageRegistration(NO_STAGE, "a", A);
    expect(applyStageRegistration(held, "a", null)).toEqual(NO_STAGE);
  });

  it("旧槽位迟到的注销被忽略——否则它会抹掉活着的那个", () => {
    // 分屏 / 剧场之间切布局：React 先挂新槽位（b），旧槽位（a）的 ref 回调随后才带着
    // `null` 到达。照单全收的话宿主从此以为自己不在回放视图——既不给后台悬浮小窗，
    // 也不给「正在播放」条，屏幕上看起来就是"切走之后什么都没了"。
    const handover = applyStageRegistration(
      applyStageRegistration(NO_STAGE, "a", A),
      "b",
      B,
    );
    expect(applyStageRegistration(handover, "a", null)).toEqual({ owner: "b", element: B });
  });

  it("换手之后原持有者再挂一次也照常接管（返回回放视图）", () => {
    const held = applyStageRegistration(NO_STAGE, "b", B);
    expect(applyStageRegistration(held, "a", A)).toEqual({ owner: "a", element: A });
  });

  it("空载时的注销是空操作，不制造新对象（避免无谓重渲染）", () => {
    expect(applyStageRegistration(NO_STAGE, "a", null)).toBe(NO_STAGE);
  });
});
