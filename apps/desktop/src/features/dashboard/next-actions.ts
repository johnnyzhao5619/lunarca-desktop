/**
 * 「下一步」栈的裁决（纯函数，可 headless 测试）。
 *
 * 概览上有三条来源会同时想说话：装机还没做完、知识库有东西待处理、库体检有发现。
 * 它们各自内部都遵守「只渲染栈顶一态、一个主按钮」，但彼此之间此前没有任何仲裁——
 * 于是三条并排，加上「继续上一次工作」和「快速开始」，一屏五个实色号召，
 * 「下一步是什么」这件事反而没人回答（PRD-07-057b 首屏优先回答「现在有什么值得继续」）。
 *
 * 这里定的就是那条仲裁：按流水线先后排序，**同屏至多一条 accent**，其余降为 muted。
 * 语气词汇本身（`NextActionTone`）跨 feature 共享，住 `lib/next-action-tone.ts`；这里只定谁拿哪一档。
 *
 * 顺序即「此刻最该做的一件事」：
 *   1. 装机（setup）—— 应用本身还没装好，谈别的都早；
 *   2. 知识库脉搏 —— 流水线上游的堆积，且它是可行动的那三态；
 *   3. 库体检 —— 「库本身有没有该收拾的」，是另一个问题，故不进脉搏的栈、只排在它后面。
 */

import type { NextActionTone } from "../../lib/next-action-tone";

/** 三条来源此刻有没有话说（体检要不要出现由它自己的取数决定，这里只问前两条）。 */
export interface NextActionPresence {
  /** 装机仍有可补齐的项。 */
  setup: boolean;
  /** 知识库脉搏处在**可行动**的态（settled / empty 只是陈述，不占主按钮）。 */
  knowledge: boolean;
}

/** 三行各自拿什么语气。第一条有话说的取 accent，其余 muted。 */
export interface NextActionTones {
  setup: NextActionTone;
  knowledge: NextActionTone;
  checkup: NextActionTone;
}

export function nextActionTones(presence: NextActionPresence): NextActionTones {
  const setup: NextActionTone = presence.setup ? "accent" : "muted";
  const knowledge: NextActionTone = !presence.setup && presence.knowledge ? "accent" : "muted";
  const checkup: NextActionTone = !presence.setup && !presence.knowledge ? "accent" : "muted";
  return { setup, knowledge, checkup };
}
