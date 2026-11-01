/**
 * 主题卡上的色块（ADR-169 ③）：一块这套主题的底色，上面是面板与一行正文、旁边是主按钮色。
 *
 * 颜色不在这里：Rust 注册表为每套主题生成 `[data-theme-swatch="<id>"]` 上的四个变量（随当前明暗取
 * 那一档，`useThemeLibrary` 装进页面），这里只给出挂变量的那个属性。纯装饰，读屏跳过。
 */
export function ThemeSwatch({ id }: { id: string }) {
  return (
    <span className="luna-theme-swatch" data-theme-swatch={id} aria-hidden="true">
      <span className="luna-theme-swatch__panel">
        <span className="luna-theme-swatch__text" />
      </span>
      <span className="luna-theme-swatch__accent" />
    </span>
  );
}
