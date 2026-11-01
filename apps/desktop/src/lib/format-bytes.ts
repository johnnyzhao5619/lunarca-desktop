/**
 * 字节量 → 带单位的可读体积串——**全应用唯一入口**（纯函数，TDD-15 §2.3：纯计算放 `lib/`）。
 *
 * **这里此前有两份**（2026-09-09 收口）：本文件一份标 `B/KB/MB/GB`、不接受 locale，
 * 供模型面板 / 语音 HUD / 首启引导 / 回收站 / 数据足迹 / 下载进度用；`lib/format-datetime.ts`
 * 里另有一份标 `B/KiB/MiB/GiB`、数值走 `Intl`，供用量统计用。两份**都按 1024 换算**，于是
 * 同一台机器上「设置 › 数据足迹」把 5 368 709 120 读成「5 GB」、「用量记录」读成「5 GiB」——
 * 同一个数字两种叫法，用户没有办法判断哪个是对的。更难堪的是后一份的注释正好在论证
 * 前一份的写法是错的。**留下的是 IEC 那一份**，理由见下。
 *
 * **进制是 IEC（1024）、单位就必须写 `KiB/MiB/GiB`**：服务端的出站额度常量是
 * `5 368 709 120 = 2³⁰ × 5`，本就是 1024 进制。按 1000 进制渲染会显示成「5.37 GB」，
 * 与运营口中的「5 GB」对不上，用户会认为被多算了；而「按 1024 算、却标 GB」是
 * Google / Microsoft / Cloudflare 三家风格指南都明令禁止的写法（`Don't use GB if you
 * mean GiB`）。两害相权，取如实标注。
 *
 * **不能用 `Intl` 的 `style: "unit"`**：它的合法单位表只有十进制那一套
 * （`kilobyte/megabyte/gigabyte`），传 `gibibyte` 直接抛 `RangeError`。故数值部分交给
 * `Intl` 本地化，单位后缀自己拼。
 *
 * `locale` 由调用方注入 `i18n.language`，绝不省略——不传读的是**系统语言**，与界面语言
 * 无关（TDD-11 §1.4b，与 `format-datetime` 同一条纪律）。
 *
 * 三位有效数字 + 不换行空格：`9.77 MiB` / `12.3 MiB` / `512 MiB`——空格是规范要求
 * （`128 GiB` 而非 `128GiB`），不换行是为了别让数字和单位被折到两行。
 */

const BYTE_UNITS = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"] as const;

/**
 * 体积**不可知**时的占位（后端没报、或拿到非有限值）。
 *
 * 「不知道多大」与「0 字节」是两件事，不能都写成 `0 B`：模型面板上「未知占用」被读成
 * 「不占空间」会让人以为删了也白删。反过来 0 也不能写成「—」——回收站空着的时候
 * 「占用 0 B」是个事实，「占用 —」看起来像出了故障。故：非有限 → 占位，0 → `0 B`。
 */
const UNKNOWN = "—";

export function formatBytes(bytes: number | null | undefined, locale: string): string {
  if (bytes == null || !Number.isFinite(bytes)) {
    return UNKNOWN;
  }
  const sign = bytes < 0 ? "-" : "";
  let n = Math.abs(bytes);
  let unit = 0;
  while (n >= 1024 && unit < BYTE_UNITS.length - 1) {
    n /= 1024;
    unit += 1;
  }
  // 字节本身不给小数（"1.00 B" 是噪音）；换算过的量给三位有效数字。
  const formatted = new Intl.NumberFormat(locale, {
    maximumSignificantDigits: unit === 0 ? 21 : 3,
  }).format(n);
  return `${sign}${formatted}\u00a0${BYTE_UNITS[unit]}`;
}

/**
 * 该维度是不是字节量——按**维度名**判定（`*_bytes`），不维护一张会漂的白名单。
 *
 * `storage_byte_hours` 刻意不在内：它的量纲是「字节 × 小时」，按字节渲染会得到一个
 * 单位错误的数字，比不换算更糟。
 */
export function isByteDimension(dimension: string): boolean {
  return dimension.endsWith("_bytes");
}
