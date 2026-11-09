/** 上次选的库（PRD-21-010）：只存库 id，不含库名或任何内容（TDD-15 §8、ADR-177 ⑤）。 */
export const LAST_VAULT_KEY = "luna:web-last-vault";

export function readLastVault(storage: Pick<Storage, "getItem">): string | null {
  const value = storage.getItem(LAST_VAULT_KEY);
  return value && /^[0-9a-f-]{36}$/i.test(value) ? value : null;
}

export function writeLastVault(storage: Pick<Storage, "setItem">, vaultId: string): void {
  storage.setItem(LAST_VAULT_KEY, vaultId);
}
