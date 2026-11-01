/**
 * Canvas 图谱标签的纯展示布局（PRD-06-034）。
 *
 * 图数据与连接度仍由 Rust 下发；这里仅把已经决定要显示的标签做屏幕空间避让，
 * 避免 Canvas 绘制顺序把几十个标题叠成一团。零 DOM 依赖，便于无头测试。
 */

export interface GraphLabelCandidate {
  id: string;
  text: string;
  centerX: number;
  top: number;
  width: number;
  height: number;
  priority: number;
  /**
   * 备选纵向位置（首选放不下时试它）。
   *
   * **没有它，加入节点避让就等于大面积吃掉标签**：真机 15 节点的图里标签从 7 个掉到
   * 3 个——密集处每个节点下方多半正压着另一个圆点，而算法此前「碰上就放弃」，连翻到
   * 上方看一眼都没有。首选恒为节点下方（阅读顺序），备选是上方。
   */
  altTop?: number;
  /** 悬停 / 键盘焦点 / 局部聚焦中心必须出现，即使附近已经有普通标签。 */
  required: boolean;
}

export interface GraphLabelPlacement extends GraphLabelCandidate {
  left: number;
}

export interface GraphLabelViewport {
  width: number;
  height: number;
  inset: number;
  collisionGap: number;
}

const LABEL_DENSITY_BREAKPOINTS = {
  /**
   * 小于这个规模的图**不设连接度门槛**——标签全给，密不密由屏幕空间碰撞避让说了算。
   *
   * 连接度门槛与碰撞避让是**两套筛法**，而后者才是按「屏幕上放不放得下」筛的那一套。
   * 节点少到本来就放得下时，前者筛掉的每一个标签都是纯损失：真机 15 节点的库里只显示
   * 了 2 个标题，另外 13 个是一堆无名圆点，而画布空着一大半——用户没法判断那些点是什么，
   * 也就没法判断这张图在说什么。取 28 而不是贴着既有的 36：36 是「密度惩罚」的起点，
   * 两件事借同一个数字会让将来调其中一个时误伤另一个。
   */
  smallGraph: 28,
  mediumGraph: 36,
  largeGraph: 72,
  overviewZoom: 0.7,
  defaultZoom: 1.1,
  detailZoom: 1.6,
  fullDetailZoom: 2.2,
} as const;

function overlaps(
  a: Pick<GraphLabelPlacement, "left" | "top" | "width" | "height">,
  b: Pick<GraphLabelPlacement, "left" | "top" | "width" | "height">,
  gap: number,
): boolean {
  return !(
    a.left + a.width + gap <= b.left ||
    b.left + b.width + gap <= a.left ||
    a.top + a.height + gap <= b.top ||
    b.top + b.height + gap <= a.top
  );
}

/**
 * 屏幕空间里的一个节点圆点——标签布局的**静态障碍物**（PRD-06-034）。
 *
 * 它不是候选项，只占地方：标签之间互相让位是一回事，标签压在**别人的圆点**上是另一
 * 回事。此前的避让只做前者，于是 A 的标题可以正好盖住 B 这个点，而算法对此一无所知
 * （真机可见）。
 *
 * **`id` 是为了让一个节点的圆不参与自己标签的判定**，这不是洁癖而是必须：标签的
 * 纵向位置就是按「圆心 + 半径 + 间隙」算出来的，于是圆心到标签边的距离**恰好等于**
 * 判定阈值 `radius + gap`。浮点运算往哪边偏是随机的——偏小一点就被判成碰撞。不排除
 * 自己的话，每个节点的标签都有约一半概率被自己的圆点挤掉，表现为标签随机消失。
 */
export interface GraphLabelObstacle {
  /** 该圆属于哪个节点（与 [`GraphLabelCandidate.id`] 同一命名空间）。 */
  id: string;
  centerX: number;
  centerY: number;
  radius: number;
}

/** 标签矩形是否与一个圆相交（先按圆的外接正方形粗筛，再算最近点距离）。 */
function hitsObstacle(
  rect: Pick<GraphLabelPlacement, "left" | "top" | "width" | "height">,
  circle: GraphLabelObstacle,
  gap: number,
): boolean {
  const reach = circle.radius + gap;
  // 矩形上离圆心最近的那个点。
  const nearestX = Math.max(rect.left, Math.min(circle.centerX, rect.left + rect.width));
  const nearestY = Math.max(rect.top, Math.min(circle.centerY, rect.top + rect.height));
  const dx = circle.centerX - nearestX;
  const dy = circle.centerY - nearestY;
  return dx * dx + dy * dy < reach * reach;
}

/**
 * 以优先级从高到低放置标签；普通标签碰撞即让位，required 标签永远保留。
 * 横向位置夹在画布内，底部放不下时翻到节点上方（调用方传入的 top 可为负）。
 *
 * `obstacles` = 屏幕空间里的全部节点圆点。缺省为空数组：本函数此前不认识圆点，留一个
 * 默认值让既有调用方与测试不必逐个改签名——**但画布必须传**，否则等于没修。
 */
export function placeGraphLabels(
  candidates: readonly GraphLabelCandidate[],
  viewport: GraphLabelViewport,
  obstacles: readonly GraphLabelObstacle[] = [],
): GraphLabelPlacement[] {
  const ordered = [...candidates].sort(
    (a, b) =>
      Number(b.required) - Number(a.required) ||
      b.priority - a.priority ||
      a.id.localeCompare(b.id),
  );
  const placed: GraphLabelPlacement[] = [];

  for (const candidate of ordered) {
    const maxLeft = Math.max(viewport.inset, viewport.width - viewport.inset - candidate.width);
    const left = Math.min(
      maxLeft,
      Math.max(viewport.inset, candidate.centerX - candidate.width / 2),
    );
    const clampTop = (value: number) =>
      Math.min(
        Math.max(viewport.inset, viewport.height - viewport.inset - candidate.height),
        Math.max(viewport.inset, value),
      );
    /** 这个位置放得下吗（既不压别的标签，也不压别人的圆点）。 */
    const fits = (rect: GraphLabelPlacement) =>
      placed.every((existing) => !overlaps(rect, existing, viewport.collisionGap)) &&
      obstacles.every(
        (circle) =>
          circle.id === candidate.id || !hitsObstacle(rect, circle, viewport.collisionGap),
      );

    // required 标签（悬停 / 键盘焦点 / 局部聚焦中心）无条件按首选位置保留：用户此刻正
    // 指着它，压住一个圆点也得显示——看不见自己指的是什么，比压住一个点更糟。
    const primary = { ...candidate, left, top: clampTop(candidate.top) };
    if (candidate.required || fits(primary)) {
      placed.push(primary);
      continue;
    }
    // 首选放不下就翻到备选（节点上方）再试一次，而不是直接丢掉这个标签。
    if (candidate.altTop !== undefined) {
      const alternate = { ...candidate, left, top: clampTop(candidate.altTop) };
      if (fits(alternate)) {
        placed.push(alternate);
      }
    }
  }

  return placed;
}

/**
 * 大图默认只给高连接度节点标题；放大后逐层显露更多细节。
 * 节点数只用于密度档位，不改变图数据、过滤或排序语义。
 */
export function ambientLabelDegreeThreshold(scale: number, nodeCount: number): number {
  // 小图无门槛：交给碰撞避让按优先级排（它本来就按 degree 排序、放不下才丢）。
  // 这一档不随缩放变化——放大一张本来就全标了的小图，没有"更多细节"可以显露。
  if (nodeCount < LABEL_DENSITY_BREAKPOINTS.smallGraph) {
    return 0;
  }
  const densityPenalty =
    nodeCount >= LABEL_DENSITY_BREAKPOINTS.largeGraph
      ? 2
      : nodeCount >= LABEL_DENSITY_BREAKPOINTS.mediumGraph
        ? 1
        : 0;
  if (scale < LABEL_DENSITY_BREAKPOINTS.overviewZoom) return 8 + densityPenalty;
  if (scale < LABEL_DENSITY_BREAKPOINTS.defaultZoom) return 4 + densityPenalty;
  if (scale < LABEL_DENSITY_BREAKPOINTS.detailZoom) return 2 + densityPenalty;
  if (scale < LABEL_DENSITY_BREAKPOINTS.fullDetailZoom) return 1;
  return 0;
}

/** 按最大像素宽度截断 Canvas 标签；按 Unicode code point 切分，避免截断代理对。 */
export function fitGraphLabel(
  label: string,
  maxWidth: number,
  measure: (value: string) => number,
): string {
  if (measure(label) <= maxWidth) return label;
  const units = Array.from(label);
  const ellipsis = "…";
  let low = 0;
  let high = units.length;
  while (low < high) {
    const mid = Math.ceil((low + high) / 2);
    if (measure(`${units.slice(0, mid).join("")}${ellipsis}`) <= maxWidth) {
      low = mid;
    } else {
      high = mid - 1;
    }
  }
  return `${units.slice(0, low).join("")}${ellipsis}`;
}
