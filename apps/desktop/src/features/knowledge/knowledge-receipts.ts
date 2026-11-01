import type { KnowledgeReceiptDto } from "../../ipc/bindings";

const MAX_RECEIPTS = 3;

/**
 * 同一操作只保留最新修订；重复事件按 receipt_id 幂等。排序新近在前并封顶，避免回执
 * 变成第二个通知中心。纯函数供宿主与守护测试共用。
 */
export function upsertKnowledgeReceipt(
  current: KnowledgeReceiptDto[],
  incoming: KnowledgeReceiptDto,
): KnowledgeReceiptDto[] {
  if (current.some((item) => item.receipt_id === incoming.receipt_id)) return current;
  const prior = current.find((item) => item.operation_id === incoming.operation_id);
  if (prior && prior.revision > incoming.revision) return current;
  return [incoming, ...current.filter((item) => item.operation_id !== incoming.operation_id)].slice(
    0,
    MAX_RECEIPTS,
  );
}

