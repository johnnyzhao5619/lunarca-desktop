/**
 * 弹出浏览器的文件选择框（「插入附件」，PRD-21-025）：选中的文件原样交回；取消给空数组。
 * 不进 DOM、不需要样式——浏览器允许对未挂载的 `<input type="file">` 调 `click()`。
 */
export function chooseFiles(): Promise<File[]> {
  return new Promise((resolve) => {
    const input = document.createElement("input");
    input.type = "file";
    input.multiple = true;
    input.addEventListener("change", () => resolve([...(input.files ?? [])]), { once: true });
    input.addEventListener("cancel", () => resolve([]), { once: true });
    input.click();
  });
}
