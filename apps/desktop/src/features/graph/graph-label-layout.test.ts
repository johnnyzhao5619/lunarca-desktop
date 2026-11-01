import { describe, expect, it } from "vitest";
import {
  ambientLabelDegreeThreshold,
  fitGraphLabel,
  placeGraphLabels,
  type GraphLabelCandidate,
} from "./graph-label-layout";

function candidate(
  id: string,
  centerX: number,
  priority: number,
  required = false,
): GraphLabelCandidate {
  return {
    id,
    text: id,
    centerX,
    top: 20,
    width: 60,
    height: 14,
    priority,
    required,
  };
}

describe("placeGraphLabels", () => {
  const viewport = { width: 240, height: 120, inset: 8, collisionGap: 4 };

  it("碰撞时保留高优先级标签并稳定排序", () => {
    const placed = placeGraphLabels(
      [candidate("low", 80, 1), candidate("high", 82, 9), candidate("far", 190, 2)],
      viewport,
    );
    expect(placed.map((entry) => entry.id)).toEqual(["high", "far"]);
  });

  it("required 标签不被普通标签遮掉，并把超出画布的文字夹回可视区", () => {
    const placed = placeGraphLabels(
      [candidate("ambient", 8, 99), candidate("focused", 8, 1, true)],
      viewport,
    );
    expect(placed.map((entry) => entry.id)).toEqual(["focused"]);
    expect(placed[0]?.left).toBe(viewport.inset);
  });
});

describe("label level of detail", () => {
  it("大图在默认缩放下提高连接度门槛，放大后逐层释放", () => {
    // 30 = 已过小图档（28）但未到密度惩罚档（36），故是原样的基础门槛。
    expect(ambientLabelDegreeThreshold(1, 30)).toBe(4);
    expect(ambientLabelDegreeThreshold(1, 80)).toBe(6);
    expect(ambientLabelDegreeThreshold(1.7, 80)).toBe(1);
    expect(ambientLabelDegreeThreshold(2.4, 80)).toBe(0);
  });

  it("小图不设连接度门槛——密不密由碰撞避让说了算", () => {
    // 连接度门槛与碰撞避让是两套筛法，后者才是按「屏幕上放不放得下」筛的那一套。
    // 节点少到本来就放得下时，前者筛掉的每一个标签都是纯损失：真机 15 节点的库里
    // 只显示了 2 个标题，另外 13 个是无名圆点，而画布空着一大半。
    expect(ambientLabelDegreeThreshold(0.5, 15)).toBe(0);
    expect(ambientLabelDegreeThreshold(1, 15)).toBe(0);
    expect(ambientLabelDegreeThreshold(1, 27)).toBe(0);
    // 缩得再小也不回退门槛——放大一张本来就全标了的小图没有更多细节可显露。
    expect(ambientLabelDegreeThreshold(0.2, 5)).toBe(0);
    // 到了中图规模，门槛照旧生效。
    expect(ambientLabelDegreeThreshold(1, 28)).toBe(4);
  });

  it("按像素宽度截断中英文标题并保留完整 Unicode 字符", () => {
    const measure = (value: string) => Array.from(value).length * 10;
    expect(fitGraphLabel("短标题", 100, measure)).toBe("短标题");
    expect(fitGraphLabel("关系图谱里的很长标题", 60, measure)).toBe("关系图谱里…");
    expect(fitGraphLabel("A😀BC", 30, measure)).toBe("A😀…");
  });
});

describe("标签避开节点圆点（PRD-06-034）", () => {
  const viewport = { width: 800, height: 600, inset: 8, collisionGap: 6 };
  const candidate = (id: string, centerX: number, top: number): GraphLabelCandidate => ({
    id,
    text: id,
    centerX,
    top,
    width: 100,
    height: 16,
    priority: 0,
    required: false,
  });

  it("压在别人圆点上的标签让位", () => {
    // 标签落在 x∈[350,450] / y∈[300,316]，圆心 (400,308) 半径 12 正在其中。
    const placed = placeGraphLabels([candidate("a", 400, 300)], viewport, [
      { id: "b", centerX: 400, centerY: 308, radius: 12 },
    ]);
    expect(placed).toHaveLength(0);
  });

  it("离得开的圆点不影响放置", () => {
    const placed = placeGraphLabels([candidate("a", 400, 300)], viewport, [
      { id: "b", centerX: 400, centerY: 500, radius: 12 },
    ]);
    expect(placed.map((p) => p.id)).toEqual(["a"]);
  });

  it("`required` 标签压住圆点也照样显示", () => {
    // 用户此刻正指着它——看不见自己指的是什么，比压住一个点更糟。
    const forced = { ...candidate("a", 400, 300), required: true };
    const placed = placeGraphLabels([forced], viewport, [
      { id: "b", centerX: 400, centerY: 308, radius: 12 },
    ]);
    expect(placed.map((p) => p.id)).toEqual(["a"]);
  });

  it("不传障碍物时行为与从前逐字一致（既有调用方不受影响）", () => {
    const placed = placeGraphLabels([candidate("a", 400, 300)], viewport);
    expect(placed.map((p) => p.id)).toEqual(["a"]);
  });

  it("只擦到圆的边缘也算碰上——判据是最近点距离，不是圆心在不在矩形里", () => {
    // 圆心在标签矩形之外（矩形右缘 450，圆心 458），但半径 12 + gap 6 够得着。
    const placed = placeGraphLabels([candidate("a", 400, 300)], viewport, [
      { id: "b", centerX: 458, centerY: 308, radius: 12 },
    ]);
    expect(placed).toHaveLength(0);
  });
});

describe("节点自己的圆不参与自己标签的判定", () => {
  const viewport = { width: 800, height: 600, inset: 8, collisionGap: 6 };

  it("自己的圆点压得再实，自己的标签也照常显示", () => {
    // 判据是「这个圆属于谁」，不是「离得够不够远」。故这里刻意把标签放到圆心上——
    // 换成别人的圆必然让位（见下一条），是自己的就必须留下。
    const own = {
      id: "a",
      text: "a",
      centerX: 400,
      top: 300,
      width: 100,
      height: 16,
      priority: 0,
      required: false,
    };
    const placed = placeGraphLabels([own], viewport, [
      { id: "a", centerX: 400, centerY: 305, radius: 12 },
    ]);
    expect(placed.map((p) => p.id)).toEqual(["a"]);
  });

  it("同样位置换成别人的圆，就必须让位（证明上一条不是因为没碰上）", () => {
    const own = {
      id: "a",
      text: "a",
      centerX: 400,
      top: 300,
      width: 100,
      height: 16,
      priority: 0,
      required: false,
    };
    const placed = placeGraphLabels([own], viewport, [
      { id: "b", centerX: 400, centerY: 305, radius: 12 },
    ]);
    expect(placed).toHaveLength(0);
  });

  it("为什么必须按 id 排除：圆心到标签边的距离恒等于判定阈值", () => {
    // 标签的纵向位置就是按「圆心 + 半径 + 间隙」算出来的，于是这个距离**恒等于**阈值
    // radius + gap。整数下 `<` 判定不碰撞，但真实 scale 是小数，浮点往哪边偏是随机的
    // ——不按 id 排除的话，每个节点的标签都有约一半概率被自己的圆点挤掉。
    const radius = 12.37;
    const centerY = 301.19;
    const own = {
      id: "a",
      text: "a",
      centerX: 400,
      top: centerY + radius + viewport.collisionGap, // 画布就是这么算的
      width: 100,
      height: 16,
      priority: 0,
      required: false,
    };
    const placed = placeGraphLabels([own], viewport, [
      { id: "a", centerX: 400, centerY, radius },
    ]);
    expect(placed.map((p) => p.id)).toEqual(["a"]);
  });
});

describe("首选放不下时翻到备选位置", () => {
  const viewport = { width: 800, height: 600, inset: 8, collisionGap: 6 };

  it("被别人的圆点挡住时，改用 altTop 而不是丢掉这个标签", () => {
    // 没有这一步，加入节点避让就等于大面积吃掉标签：密集处每个节点下方多半正压着
    // 另一个圆点，而算法「碰上就放弃」，连翻到上方看一眼都没有。
    const placed = placeGraphLabels(
      [
        {
          id: "a",
          text: "a",
          centerX: 400,
          top: 300,
          altTop: 100,
          width: 100,
          height: 16,
          priority: 0,
          required: false,
        },
      ],
      viewport,
      [{ id: "b", centerX: 400, centerY: 305, radius: 12 }],
    );
    expect(placed.map((p) => p.top)).toEqual([100]);
  });

  it("两个位置都被挡住才真的放弃", () => {
    const placed = placeGraphLabels(
      [
        {
          id: "a",
          text: "a",
          centerX: 400,
          top: 300,
          altTop: 100,
          width: 100,
          height: 16,
          priority: 0,
          required: false,
        },
      ],
      viewport,
      [
        { id: "b", centerX: 400, centerY: 305, radius: 12 },
        { id: "c", centerX: 400, centerY: 105, radius: 12 },
      ],
    );
    expect(placed).toHaveLength(0);
  });

  it("没给 altTop 的候选维持原行为（碰上即让位）", () => {
    const placed = placeGraphLabels(
      [
        {
          id: "a",
          text: "a",
          centerX: 400,
          top: 300,
          width: 100,
          height: 16,
          priority: 0,
          required: false,
        },
      ],
      viewport,
      [{ id: "b", centerX: 400, centerY: 305, radius: 12 }],
    );
    expect(placed).toHaveLength(0);
  });
});
