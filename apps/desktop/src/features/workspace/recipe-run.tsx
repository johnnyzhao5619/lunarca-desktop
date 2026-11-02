/**
 * 运行一份工作流配方（纵切 7b B5，ADR-161 ② ③，PRD-18-010）：确认框逐条写它会做的事，确认后交给 Rust 跑。
 *
 * 判定都在 Rust：哪些配方能运行、接在哪里、每一步是什么、会做哪些事（`runnable_recipes`）；运行时 Rust 把这里交回的
 * 能力与此刻推导出的再比一遍，包在确认之后变了就要重新确认（`RECIPE_CONFIRM_AGAIN`）。这里只摆字、转发、报结果。
 */

import { createElement } from "react";

import type { ConfirmFn } from "../../components/ConfirmHost";
import { commands, type RecipeDto, type RecipeStepDto } from "../../ipc/bindings";
import { openNote } from "../../lib/navigate";
import { langLabelKey } from "../settings/language-options";

/** 带命名空间前缀调用的取词器（与 `note-menu` 的 `MenuTranslate` 同形；不从那里导入，免得两个文件互相依赖）。 */
type MenuTranslate = (key: string, params?: Record<string, unknown>) => string;

/** 一步一句。 */
export function recipeStepLine(step: RecipeStepDto, t: MenuTranslate): string {
  switch (step.type) {
    case "transcribe":
      return t("workspace:recipes.step.transcribe");
    case "translate":
      return t("workspace:recipes.step.translate", { lang: t(langLabelKey(step.to)) });
    case "llm":
      return t("workspace:recipes.step.llm", { prompt: step.prompt });
    case "vault_create_note":
      return t("workspace:recipes.step.createNote", { dir: step.dir });
    case "export":
      return t("workspace:recipes.step.export", { format: step.format.toUpperCase() });
  }
}

function RecipeConfirm({ recipe, t }: { recipe: RecipeDto; t: MenuTranslate }) {
  return (
    <div className="luna-apps__confirm">
      <p>{t("workspace:recipes.from", { pack: recipe.pack_name, version: recipe.pack_version })}</p>
      <p className="luna-apps__confirm-lead">{t("workspace:recipes.stepsLead")}</p>
      <ol className="luna-apps__confirm-list" aria-label={t("workspace:recipes.stepsLead")}>
        {recipe.steps.map((step, index) => (
          <li key={index}>{recipeStepLine(step, t)}</li>
        ))}
      </ol>
      <p className="luna-apps__note">{t("workspace:recipes.note")}</p>
    </div>
  );
}

/** 运行失败的稳定错误码 → 状态栏那句话。 */
export function recipeErrorText(error: string, t: MenuTranslate): string {
  const code = error.split(":", 1)[0];
  switch (code) {
    case "RECIPE_NO_TRANSCRIPT":
      return t("workspace:recipes.error.noTranscript");
    case "RECIPE_WRONG_INPUT":
      return t("workspace:recipes.error.wrongInput");
    case "RECIPE_EMPTY_INPUT":
      return t("workspace:recipes.error.empty");
    case "RECIPE_INPUT_TOO_LONG":
      return t("workspace:recipes.error.tooLong");
    case "RECIPE_CONFIRM_AGAIN":
      return t("workspace:recipes.error.confirmAgain");
    case "RECIPE_NOT_FOUND":
    case "RECIPE_NOT_RUNNABLE":
      return t("workspace:recipes.error.gone");
    case "RECIPE_UNKNOWN_FORMAT":
    case "RECIPE_EXPORT_NEEDS_TIMING":
      return t("workspace:recipes.error.export");
    default:
      return t("workspace:recipes.error.failed");
  }
}

/**
 * 确认 → 运行 → 报结果。新建了笔记就打开它。
 */
export async function runRecipe(
  path: string,
  recipe: RecipeDto,
  t: MenuTranslate,
  ask: ConfirmFn,
  onStatus: (message: string) => void,
): Promise<void> {
  const ok = await ask({
    title: t("workspace:recipes.confirmTitle", { title: recipe.title }),
    content: createElement(RecipeConfirm, { recipe, t }),
    confirmLabel: t("workspace:recipes.confirm"),
    destructive: false,
  });
  if (!ok) return;
  onStatus(t("workspace:recipes.running", { title: recipe.title }));
  const result = await commands.runRecipe(path, recipe.pack, recipe.id, recipe.capabilities);
  if (result.status === "error") {
    onStatus(recipeErrorText(result.error, t));
    return;
  }
  onStatus(t("workspace:recipes.done", { title: recipe.title }));
  if (result.data.created !== null) openNote(result.data.created);
}
