// Что умеет платформа, на которой мы запущены.
//
// Одно место правды вместо проверок по месту. Десктоп и телефон отличаются не
// оформлением, а возможностями: на телефоне нет отдельных окон, нет полного
// экрана окна, а обновления ставит магазин, а не мы. Код, который зовёт такие
// вещи, спрашивает здесь — и на телефоне просто выбирает другой путь.

const agent = typeof navigator === 'undefined' ? '' : navigator.userAgent;

/** Телефон или планшет. iPad притворяется маком, поэтому смотрим и на касание. */
export const mobile: boolean =
  /Android|iPhone|iPod/i.test(agent) ||
  (/Macintosh/i.test(agent) && typeof navigator !== 'undefined' && navigator.maxTouchPoints > 1);

/** Внутри приложения, а не на демонстрационном стенде в браузере. */
export const native: boolean =
  typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window && !('__BRED_DEMO__' in window);

export const platform = {
  mobile,
  native,
  /** Отдельные окна (бой с Гундиром) — только на десктопе. */
  windows: native && !mobile,
  /** Полный экран самого окна для чужой демонстрации. */
  fullscreen: native && !mobile,
  /** Встроенные обновления: на телефоне их ставит магазин. */
  updater: native && !mobile,
} as const;
