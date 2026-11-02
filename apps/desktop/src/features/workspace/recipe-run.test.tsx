// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import type { RecipeDto } from "../../ipc/bindings";
import { NAVIGATE_EVENT } from "../../lib/navigate";
import { recipeErrorText, recipeStepLine, runRecipe } from "./recipe-run";

const t = (key: string, params?: Record<string, unknown>) => (params ? `${key} ${JSON.stringify(params)}` : key);

const RECIPE: RecipeDto = {
  pack: "app.lunarca.creator-starter",
  pack_name: "创作者起步包",
  pack_version: "1.1.0",
  id: "app.lunarca.creator-starter.package",
  title: "发布包",
  input: "recording",
  steps: [
    { type: "transcribe" },
    { type: "llm", prompt: "publish-package" },
    { type: "vault_create_note", dir: "Projects" },
  ],
  capabilities: { uses_ai: true, transcribes: true, translates: false, creates_notes_in: ["Projects"], exports: false },
};

function recordRoutes() {
  const list: string[] = [];
  const listener = (event: Event) => {
    list.push(String((event as CustomEvent).detail));
    event.preventDefault();
  };
  window.addEventListener(NAVIGATE_EVENT, listener);
  return { list, stop: () => window.removeEventListener(NAVIGATE_EVENT, listener) };
}

beforeEach(() => resetBindingsDouble());
afterEach(() => vi.restoreAllMocks());

describe("运行工作流（纵切 7b B5）", () => {
  it("确认后把 Rust 给的能力原样交回；新建了笔记就打开它", async () => {
    setCommandReply("runRecipe", { status: "ok", data: { created: "Projects/访谈 · 发布包.md", exported: null } });
    const routes = recordRoutes();
    const status = vi.fn();
    await runRecipe("Recordings/访谈.md", RECIPE, t, async () => true, status);
    routes.stop();
    expect(commandMock("runRecipe")).toHaveBeenCalledWith(
      "Recordings/访谈.md",
      RECIPE.pack,
      RECIPE.id,
      RECIPE.capabilities,
    );
    expect(status).toHaveBeenLastCalledWith(`workspace:recipes.done ${JSON.stringify({ title: "发布包" })}`);
    expect(routes.list.some((route) => route.includes(encodeURIComponent("Projects/访谈 · 发布包.md")) || route.includes("Projects/访谈 · 发布包.md"))).toBe(true);
  });

  it("取消就什么都不发；失败按码说清楚", async () => {
    await runRecipe("Notes/a.md", RECIPE, t, async () => false, vi.fn());
    expect(commandMock("runRecipe")).not.toHaveBeenCalled();
    setCommandReply("runRecipe", { status: "error", error: "RECIPE_NO_TRANSCRIPT" });
    const status = vi.fn();
    await runRecipe("Notes/a.md", RECIPE, t, async () => true, status);
    expect(status).toHaveBeenLastCalledWith("workspace:recipes.error.noTranscript");
    expect(recipeErrorText("RECIPE_INPUT_TOO_LONG: 9 > 3", t)).toBe("workspace:recipes.error.tooLong");
    expect(recipeErrorText("SOMETHING_ELSE", t)).toBe("workspace:recipes.error.failed");
  });

  it("每一步一句话", () => {
    expect(recipeStepLine({ type: "vault_create_note", dir: "Projects" }, t)).toBe(
      `workspace:recipes.step.createNote ${JSON.stringify({ dir: "Projects" })}`,
    );
    expect(recipeStepLine({ type: "export", format: "srt" }, t)).toBe(
      `workspace:recipes.step.export ${JSON.stringify({ format: "SRT" })}`,
    );
  });
});
