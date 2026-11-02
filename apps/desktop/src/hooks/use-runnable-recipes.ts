/**
 * 此刻能运行的工作流配方（纵切 7b B5，ADR-161 ②）：Rust `runnable_recipes` 的投影。
 *
 * 哪些能运行（只有从在线目录装的、没被封禁的包）、每一份接在录音还是笔记上、会做哪些事，都由 Rust 算好；命令零网络。
 * 装上、更新、卸载或封禁之后随 `templates://changed` 重读。
 */

import { useEffect, useState } from "react";

import { commands, events, type RecipeDto } from "../ipc/bindings";

export function useRunnableRecipes(): readonly RecipeDto[] {
  const [recipes, setRecipes] = useState<RecipeDto[]>([]);
  useEffect(() => {
    let alive = true;
    const reload = () => {
      void commands.runnableRecipes().then((list) => {
        if (alive) setRecipes(list);
      });
    };
    reload();
    const unlisten = events.templatesChanged.listen(() => reload());
    return () => {
      alive = false;
      void unlisten.then((dispose) => dispose());
    };
  }, []);
  return recipes;
}
