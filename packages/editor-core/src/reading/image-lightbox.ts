/**
 * 看大图（对标方案 D4；Obsidian 1.13 的图片灯箱）：整窗遮罩里按窗口大小显示图片，←/→ 在同一篇
 * 的图片之间切换，Esc 或点遮罩关闭，关闭后焦点回到打开前的位置。
 *
 * 纯 DOM（不依赖 React）：编辑面的图片部件是手写 DOM，阅读面是 React，两边调同一个函数。
 */

export interface LightboxImage {
  src: string;
  alt: string;
}

export interface LightboxLabels {
  dialog: string;
  close: string;
  previous: string;
  next: string;
}

export function openImageLightbox(images: LightboxImage[], start: number, labels: LightboxLabels): void {
  if (images.length === 0) return;
  let index = Math.max(0, Math.min(images.length - 1, start));
  const restore = document.activeElement instanceof HTMLElement ? document.activeElement : null;

  const overlay = document.createElement("div");
  overlay.className = "luna-lightbox";
  overlay.setAttribute("role", "dialog");
  overlay.setAttribute("aria-modal", "true");
  overlay.setAttribute("aria-label", labels.dialog);
  overlay.tabIndex = -1;

  const img = document.createElement("img");
  img.className = "luna-lightbox__image";
  const caption = document.createElement("p");
  caption.className = "luna-lightbox__caption";

  const button = (className: string, label: string, text: string, run: () => void) => {
    const element = document.createElement("button");
    element.type = "button";
    element.className = `luna-lightbox__button ${className}`;
    element.setAttribute("aria-label", label);
    element.title = label;
    element.textContent = text;
    element.addEventListener("click", (event) => {
      event.stopPropagation();
      run();
    });
    return element;
  };

  const show = () => {
    const current = images[index];
    img.src = current.src;
    img.alt = current.alt;
    caption.textContent = images.length > 1 ? `${current.alt ? `${current.alt} · ` : ""}${index + 1} / ${images.length}` : current.alt;
    caption.hidden = caption.textContent === "";
  };
  const step = (delta: number) => {
    index = (index + delta + images.length) % images.length;
    show();
  };
  const close = () => {
    document.removeEventListener("keydown", onKey, true);
    overlay.remove();
    restore?.focus();
  };
  const onKey = (event: KeyboardEvent) => {
    if (event.key === "Escape") close();
    else if (event.key === "ArrowLeft" && images.length > 1) step(-1);
    else if (event.key === "ArrowRight" && images.length > 1) step(1);
    else return;
    event.preventDefault();
    event.stopPropagation();
  };

  overlay.append(img, caption, button("luna-lightbox__close", labels.close, "×", close));
  if (images.length > 1) {
    overlay.append(
      button("luna-lightbox__previous", labels.previous, "‹", () => step(-1)),
      button("luna-lightbox__next", labels.next, "›", () => step(1)),
    );
  }
  overlay.addEventListener("click", (event) => {
    if (event.target === overlay) close();
  });
  document.addEventListener("keydown", onKey, true);
  show();
  document.body.appendChild(overlay);
  overlay.focus();
}
