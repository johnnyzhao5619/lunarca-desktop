// 关系图谱纯逻辑无头测试（PRD-06-034）：过滤语义 / 邻域 / 布局确定性与收敛。

import { describe, expect, it } from "vitest";
import type { GraphEdgeDto, GraphNodeDto } from "../../ipc/bindings";
import {
  buildAdjacency,
  DEFAULT_GRAPH_FILTER,
  DEFAULT_SIM_PARAMS,
  filterGraph,
  fitTransformFor,
  hitTest,
  initialPosition,
  neighborhood,
  nodeRadius,
  relationsOf,
  settleSynchronously,
  simulationStep,
  toSimNodes,
  diagnoseViewport,
} from "./graph-sim";

function node(id: string, kind = "note", degree = 0, draft = false): GraphNodeDto {
  return {
    id,
    label: id,
    kind,
    note_type: kind === "note" ? "note" : null,
    draft,
    degree,
  };
}

function edge(source: string, target: string, kind = "link", strength = 100): GraphEdgeDto {
  return { source, target, kind, strength };
}

const NODES: GraphNodeDto[] = [
  node("a.md", "note", 2),
  node("b.md", "note", 2),
  node("c.md", "note", 1),
  node("tag:工作", "tag", 2),
  node("ghost:missing", "ghost", 1),
  node("orphan.md", "note", 0),
];

const EDGES: GraphEdgeDto[] = [
  edge("a.md", "b.md"),
  edge("a.md", "ghost:missing"),
  edge("a.md", "tag:工作", "tag"),
  edge("c.md", "tag:工作", "tag"),
];

describe("filterGraph", () => {
  it("默认过滤器全量保留", () => {
    const out = filterGraph(NODES, EDGES, DEFAULT_GRAPH_FILTER);
    expect(out.nodes).toHaveLength(NODES.length);
    expect(out.edges).toHaveLength(EDGES.length);
  });

  it("关闭标签节点连带裁剪标签边", () => {
    const out = filterGraph(NODES, EDGES, { ...DEFAULT_GRAPH_FILTER, showTags: false });
    expect(out.nodes.some((n) => n.kind === "tag")).toBe(false);
    expect(out.edges.some((e) => e.kind === "tag")).toBe(false);
    expect(out.edges).toHaveLength(2);
  });

  it("关闭幽灵节点连带裁剪其边", () => {
    const out = filterGraph(NODES, EDGES, { ...DEFAULT_GRAPH_FILTER, showGhosts: false });
    expect(out.nodes.some((n) => n.kind === "ghost")).toBe(false);
    expect(out.edges).toHaveLength(3);
  });

  it("关闭孤立节点仅移除无可见边的节点", () => {
    const out = filterGraph(NODES, EDGES, { ...DEFAULT_GRAPH_FILTER, showOrphans: false });
    expect(out.nodes.some((n) => n.id === "orphan.md")).toBe(false);
    expect(out.nodes.some((n) => n.id === "c.md")).toBe(true);
  });

  it("资料区笔记默认不入图，勾选后出现（ADR-024）", () => {
    const withMaterial: GraphNodeDto[] = [
      ...NODES,
      { id: "会议.md", label: "会议", kind: "note", note_type: "recording", draft: false, degree: 1 },
      { id: "视频.md", label: "视频", kind: "note", note_type: "transcript", draft: false, degree: 0 },
      { id: "对话.md", label: "对话", kind: "note", note_type: "chat", draft: false, degree: 0 },
      { id: "速记.md", label: "速记", kind: "note", note_type: "quick-note", draft: false, degree: 0 },
    ];
    const edgesWith = [...EDGES, edge("a.md", "会议.md")];

    const shown = filterGraph(withMaterial, edgesWith, DEFAULT_GRAPH_FILTER).nodes.map((n) => n.id);
    expect(shown).toContain("速记.md"); // 速记不是资料，照常入图
    expect(shown).not.toContain("会议.md");
    expect(shown).not.toContain("视频.md");
    expect(shown).not.toContain("对话.md");

    // 隐藏节点的关联边一并隐藏——绝不留悬垂边（一端不存在的边会画到画布外）。
    const defaultEdges = filterGraph(withMaterial, edgesWith, DEFAULT_GRAPH_FILTER).edges;
    expect(defaultEdges.some((e) => e.target === "会议.md")).toBe(false);

    const opened = filterGraph(withMaterial, edgesWith, {
      ...DEFAULT_GRAPH_FILTER,
      noteTypes: [...DEFAULT_GRAPH_FILTER.noteTypes, "recording"],
    });
    expect(opened.nodes.map((n) => n.id)).toContain("会议.md");
    expect(opened.edges.some((e) => e.target === "会议.md")).toBe(true);
  });

  it("缺省 note_type 的笔记按 note 计（与索引缺省口径一致）", () => {
    const legacy: GraphNodeDto[] = [
      { id: "旧.md", label: "旧", kind: "note", note_type: null, draft: false, degree: 0 },
    ];
    const out = filterGraph(legacy, [], DEFAULT_GRAPH_FILTER);
    expect(out.nodes).toHaveLength(1);
  });

  it("搜索保留命中节点及其直接邻居", () => {
    const out = filterGraph(NODES, EDGES, { ...DEFAULT_GRAPH_FILTER, query: "c.md" });
    const ids = out.nodes.map((n) => n.id).sort();
    expect(ids).toEqual(["c.md", "tag:工作"]);
    expect(out.edges).toHaveLength(1);
  });

  it("局部聚焦按跳数裁剪（depth 1 vs 2）", () => {
    const one = filterGraph(NODES, EDGES, {
      ...DEFAULT_GRAPH_FILTER,
      focus: { id: "b.md", depth: 1 },
    });
    expect(one.nodes.map((n) => n.id).sort()).toEqual(["a.md", "b.md"]);
    const two = filterGraph(NODES, EDGES, {
      ...DEFAULT_GRAPH_FILTER,
      focus: { id: "b.md", depth: 2 },
    });
    expect(two.nodes.map((n) => n.id).sort()).toEqual([
      "a.md",
      "b.md",
      "ghost:missing",
      "tag:工作",
    ]);
  });

  it("聚焦 + 关标签：不会经标签节点跳到无关笔记", () => {
    const out = filterGraph(NODES, EDGES, {
      ...DEFAULT_GRAPH_FILTER,
      showTags: false,
      focus: { id: "c.md", depth: 2 },
    });
    // c.md 只经 tag:工作 与 a.md 相连；标签关掉后聚焦区只剩自己。
    expect(out.nodes.map((n) => n.id)).toEqual(["c.md"]);
  });

  it("聚焦中心即使孤立也不被孤立过滤移除", () => {
    const out = filterGraph(NODES, EDGES, {
      ...DEFAULT_GRAPH_FILTER,
      showOrphans: false,
      focus: { id: "orphan.md", depth: 1 },
    });
    expect(out.nodes.map((n) => n.id)).toEqual(["orphan.md"]);
  });
});

describe("neighborhood / adjacency", () => {
  it("BFS 跳数语义正确且含中心", () => {
    const adjacency = buildAdjacency(EDGES);
    expect([...neighborhood(adjacency, "b.md", 1)].sort()).toEqual(["a.md", "b.md"]);
    expect(neighborhood(adjacency, "b.md", 2).size).toBe(4);
    // 图外节点：仅中心自身。
    expect([...neighborhood(adjacency, "nowhere", 3)]).toEqual(["nowhere"]);
  });
});

describe("layout", () => {
  it("初始位置确定性且互不重叠", () => {
    const a1 = initialPosition("a.md", 0);
    const a2 = initialPosition("a.md", 0);
    expect(a1).toEqual(a2);
    const b = initialPosition("b.md", 1);
    expect(Math.hypot(a1.x - b.x, a1.y - b.y)).toBeGreaterThan(1);
  });

  it("toSimNodes 保留旧位置（重载不跳图）并初始化新节点", () => {
    const first = toSimNodes(NODES, new Map());
    first[0].x = 123;
    first[0].y = -45;
    const prev = new Map(first.map((n) => [n.id, n]));
    const reloaded = toSimNodes([...NODES, node("new.md")], prev);
    expect(reloaded[0].x).toBe(123);
    expect(reloaded[0].y).toBe(-45);
    const fresh = reloaded.find((n) => n.id === "new.md");
    expect(fresh).toBeDefined();
    expect(Number.isFinite(fresh?.x)).toBe(true);
  });

  it("弹簧把相连节点拉近；能量随迭代衰减收敛", () => {
    const nodes = toSimNodes([node("a.md"), node("b.md")], new Map());
    nodes[0].x = -400;
    nodes[0].y = 0;
    nodes[1].x = 400;
    nodes[1].y = 0;
    const edges = [edge("a.md", "b.md")];
    const before = Math.abs(nodes[0].x - nodes[1].x);
    let energy = Number.POSITIVE_INFINITY;
    let alpha = 1;
    for (let i = 0; i < 300; i += 1) {
      energy = simulationStep(nodes, edges, DEFAULT_SIM_PARAMS, alpha);
      alpha = Math.max(0.02, alpha * 0.98);
    }
    const after = Math.abs(nodes[0].x - nodes[1].x);
    expect(after).toBeLessThan(before);
    expect(energy).toBeLessThan(1);
    expect(Number.isFinite(nodes[0].x)).toBe(true);
  });

  it("主题级网络里关联强的两页拉得更近（PRD-16-062：枢纽来源只给弱边）", () => {
    const nodes = toSimNodes([node("a.md", "topic"), node("b.md", "topic"), node("c.md", "topic")], new Map());
    const edges = [
      edge("a.md", "b.md", "source", 100),
      edge("a.md", "c.md", "source", 15),
      edge("b.md", "c.md", "source", 15),
    ];
    settleSynchronously(nodes, edges, DEFAULT_SIM_PARAMS, 1);
    const distance = (p: (typeof nodes)[number], q: (typeof nodes)[number]) =>
      Math.hypot(p.x - q.x, p.y - q.y);
    expect(distance(nodes[0], nodes[1])).toBeLessThan(distance(nodes[0], nodes[2]));
    expect(distance(nodes[0], nodes[1])).toBeLessThan(distance(nodes[1], nodes[2]));
  });

  it("重叠节点被确定性推开（无 NaN）", () => {
    const nodes = toSimNodes([node("a.md"), node("b.md")], new Map());
    nodes[1].x = nodes[0].x;
    nodes[1].y = nodes[0].y;
    simulationStep(nodes, [], DEFAULT_SIM_PARAMS, 1);
    expect(Number.isFinite(nodes[0].x)).toBe(true);
    expect(nodes[0].x !== nodes[1].x || nodes[0].y !== nodes[1].y).toBe(true);
  });

  it("pinned 节点不被力移动", () => {
    const nodes = toSimNodes([node("a.md"), node("b.md")], new Map());
    nodes[0].pinned = true;
    const { x, y } = nodes[0];
    simulationStep(nodes, [edge("a.md", "b.md")], DEFAULT_SIM_PARAMS, 1);
    expect(nodes[0].x).toBe(x);
    expect(nodes[0].y).toBe(y);
  });
});

describe("rendering helpers", () => {
  it("节点半径随连接度增长且有上限", () => {
    expect(nodeRadius({ degree: 0, kind: "note" })).toBeLessThan(
      nodeRadius({ degree: 9, kind: "note" }),
    );
    expect(nodeRadius({ degree: 10_000, kind: "note" })).toBeLessThanOrEqual(13);
  });

  it("hitTest 命中最近节点、超出容差返回 null", () => {
    const nodes = toSimNodes([node("a.md"), node("b.md")], new Map());
    nodes[0].x = 0;
    nodes[0].y = 0;
    nodes[1].x = 30;
    nodes[1].y = 0;
    expect(hitTest(nodes, 2, 1, 4)?.id).toBe("a.md");
    expect(hitTest(nodes, 28, 0, 4)?.id).toBe("b.md");
    expect(hitTest(nodes, 15, 15, 4)).toBeNull();
  });
});

describe("记录态开关（PRD-06-052 / ADR-086 成熟度轴）", () => {
  const DRAFTS: GraphNodeDto[] = [
    node("知识.md", "note", 1),
    node("草稿.md", "note", 1, true),
    { ...node("速记.md", "note", 0, true), note_type: "quick-note" },
    node("tag:工作", "tag", 1),
  ];
  const DRAFT_EDGES: GraphEdgeDto[] = [edge("知识.md", "草稿.md")];

  it("默认隐藏记录态（`DEFAULT_GRAPH_FILTER.showDrafts === false`），一键可开", () => {
    expect(DEFAULT_GRAPH_FILTER.showDrafts).toBe(false);
    const hidden = filterGraph(DRAFTS, DRAFT_EDGES, DEFAULT_GRAPH_FILTER);
    expect(hidden.nodes.map((n) => n.id)).toEqual(["知识.md", "tag:工作"]);
    const shown = filterGraph(DRAFTS, DRAFT_EDGES, {
      ...DEFAULT_GRAPH_FILTER,
      showDrafts: true,
    });
    expect(shown.nodes).toHaveLength(4);
  });

  it("隐藏记录连带裁掉只连着记录的边（与资料的类型过滤同口径）", () => {
    const out = filterGraph(DRAFTS, DRAFT_EDGES, DEFAULT_GRAPH_FILTER);
    expect(out.edges).toHaveLength(0);
  });

  it("成熟度与类型**正交**：打开「记录」不会把被类型开关关掉的记录带回来", () => {
    // 这条锁的正是"记录是第六种类型"那个误读——若两者写成同一个维度，
    // 下面这个只勾了 `note` 的过滤器会连速记一起放进来。
    const out = filterGraph(DRAFTS, DRAFT_EDGES, {
      ...DEFAULT_GRAPH_FILTER,
      noteTypes: ["note"],
      showDrafts: true,
    });
    expect(out.nodes.map((n) => n.id)).toEqual(["知识.md", "草稿.md", "tag:工作"]);
  });

  it("标签与幽灵节点恒非记录态，不受这个开关影响", () => {
    const out = filterGraph(DRAFTS, DRAFT_EDGES, DEFAULT_GRAPH_FILTER);
    expect(out.nodes.some((n) => n.kind === "tag")).toBe(true);
  });
});

describe("fitTransformFor：初次进入把整张图放进视口", () => {
  it("把包围盒中心送到画布中心，并按较紧的一轴定缩放", () => {
    // 画布 1000×600，留白各 72 → 可用 856×456。跨度 400×200 时较紧的一轴是 x：
    // 856/400 = 2.14，但放大有上限（见下一条），故这里给一个会真正落在区间内的跨度。
    const nodes = [
      { x: -500, y: -250 },
      { x: 500, y: 250 },
    ];
    const fit = fitTransformFor(nodes, 1000, 600, 0.2);
    // 跨度 1000×500：x 轴 856/1000 = 0.856，y 轴 456/500 = 0.912 → 取较小者。
    expect(fit?.scale).toBeCloseTo(0.856, 3);
    // 包围盒中心是原点，故不需要平移。
    expect(fit?.x).toBeCloseTo(0, 6);
    expect(fit?.y).toBeCloseTo(0, 6);
  });

  it("只缩小、放大有上限——三节点的图不该被撑成气泡", () => {
    const nodes = [
      { x: -10, y: -10 },
      { x: 10, y: 10 },
    ];
    const fit = fitTransformFor(nodes, 1000, 600, 0.2);
    expect(fit?.scale).toBe(2.5);
  });

  it("偏心的图会被平移回中心", () => {
    const nodes = [
      { x: 800, y: 400 },
      { x: 1200, y: 600 },
    ];
    const fit = fitTransformFor(nodes, 1000, 600, 0.2);
    const scale = fit?.scale ?? 0;
    // 中心 (1000, 500) → pan = -center * scale。
    expect(fit?.x).toBeCloseTo(-1000 * scale, 6);
    expect(fit?.y).toBeCloseTo(-500 * scale, 6);
  });

  it("单节点不产生除零，落在放大上限上并居中", () => {
    const fit = fitTransformFor([{ x: 40, y: -20 }], 1000, 600, 0.2);
    expect(fit?.scale).toBe(2.5);
    expect(fit?.x).toBeCloseTo(-40 * 2.5, 6);
    expect(fit?.y).toBeCloseTo(20 * 2.5, 6);
  });

  it("没有节点或视口无效时返回 null——调用方保持当前变换不动", () => {
    expect(fitTransformFor([], 1000, 600, 0.2)).toBeNull();
    expect(fitTransformFor([{ x: 0, y: 0 }], 0, 600, 0.2)).toBeNull();
  });

  it("仿真尚未产生有效位置的节点不参与包围盒", () => {
    // 一个 NaN 混进 min/max 会把整个包围盒变成 NaN，画布随之整屏空白。
    const fit = fitTransformFor(
      [{ x: Number.NaN, y: 0 }, { x: -500, y: -250 }, { x: 500, y: 250 }],
      1000,
      600,
      0.2,
    );
    expect(fit?.scale).toBeCloseTo(0.856, 3);
  });
});

describe("关系列表与低动态效果（K7）", () => {
  it("关系按种类与名称列出，只转述边的种类；同一邻居同一种关系只列一次", () => {
    const nodes = [node("a.md"), node("b.md"), node("c.md"), node("tag:项目", "tag")];
    const edges = [
      edge("a.md", "b.md"),
      edge("a.md", "b.md"),
      edge("c.md", "a.md"),
      edge("a.md", "tag:项目", "tag"),
      edge("b.md", "tag:项目", "tag"),
    ];
    expect(relationsOf("a.md", nodes, edges).map((r) => [r.kind, r.id])).toEqual([
      ["links_to", "b.md"],
      ["linked_from", "c.md"],
      ["tag", "tag:项目"],
    ]);
    expect(relationsOf("tag:项目", nodes, edges).map((r) => [r.kind, r.id])).toEqual([
      ["tagged_note", "a.md"],
      ["tagged_note", "b.md"],
    ]);
  });

  it("减少动态效果时一次算到静止，步数有上限", () => {
    const nodes = toSimNodes([node("a.md"), node("b.md"), node("c.md")], new Map());
    const edges = [edge("a.md", "b.md"), edge("b.md", "c.md")];
    expect(settleSynchronously(nodes, edges, DEFAULT_SIM_PARAMS, 1)).toBe(0);
    // 已经静止的布局再算一次几乎不再移动。
    const before = nodes.map((n) => [n.x, n.y]);
    simulationStep(nodes, edges, DEFAULT_SIM_PARAMS, 0.02);
    const drift = nodes.reduce(
      (sum, n, i) => sum + Math.hypot(n.x - before[i][0], n.y - before[i][1]),
      0,
    );
    expect(drift).toBeLessThan(1);
  });
});

describe("diagnoseViewport（K7 空白取证）", () => {
  const nodes = [
    { x: 0, y: 0 },
    { x: 100, y: 0 },
  ];
  it("按绘制同一换算数可见节点；平移出界后一个都看不见", () => {
    expect(diagnoseViewport(nodes, { x: 0, y: 0, scale: 1 }, 400, 300)).toMatchObject({
      total: 2,
      visible: 2,
      nonFinite: 0,
      bbox: { minX: 0, maxX: 100, minY: 0, maxY: 0 },
    });
    expect(diagnoseViewport(nodes, { x: 5000, y: 0, scale: 1 }, 400, 300).visible).toBe(0);
  });
  it("非有限坐标与非有限平移缩放如实计数，不算可见", () => {
    const broken = [...nodes, { x: Number.NaN, y: 0 }];
    const diagnosis = diagnoseViewport(broken, { x: 0, y: 0, scale: 1 }, 400, 300);
    expect(diagnosis.nonFinite).toBe(1);
    expect(diagnosis.visible).toBe(2);
    expect(diagnoseViewport(nodes, { x: Number.NaN, y: 0, scale: 1 }, 400, 300).visible).toBe(0);
    expect(diagnoseViewport([{ x: Number.NaN, y: Number.NaN }], { x: 0, y: 0, scale: 1 }, 400, 300).bbox).toBeNull();
  });
});

