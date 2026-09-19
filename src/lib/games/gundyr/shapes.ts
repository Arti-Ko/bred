// Геометрия арены: фигуры, попадания, границы.
//
// Одна и та же фигура и показывается на полу, и наносит урон — иначе бой
// становится нечестным, а к первому боссу приходят как раз учиться читать
// замахи, а не угадывать.

export interface Vec {
  x: number;
  y: number;
}

/** Опасная зона. */
export type Shape =
  | { kind: 'circle'; x: number; y: number; r: number }
  | { kind: 'arc'; x: number; y: number; r: number; facing: number; spread: number }
  | { kind: 'line'; x: number; y: number; facing: number; len: number; half: number };

export const ARENA = { x: 460, y: 300, r: 250 };

export const len = (v: Vec): number => Math.hypot(v.x, v.y);
export const sub = (a: Vec, b: Vec): Vec => ({ x: a.x - b.x, y: a.y - b.y });
export const angleTo = (from: Vec, to: Vec): number => Math.atan2(to.y - from.y, to.x - from.x);

/** Кратчайший поворот между углами: иначе босс разворачивается «через спину». */
export function turnTowards(from: number, to: number, limit: number): number {
  let delta = ((to - from + Math.PI * 3) % (Math.PI * 2)) - Math.PI;
  if (delta > limit) delta = limit;
  if (delta < -limit) delta = -limit;
  return from + delta;
}

export function insideShape(point: Vec, r: number, shape: Shape): boolean {
  if (shape.kind === 'circle') {
    return len(sub(point, shape)) <= shape.r + r;
  }
  if (shape.kind === 'arc') {
    const distance = len(sub(point, shape));
    if (distance > shape.r + r) return false;
    const delta = Math.abs(
      ((angleTo(shape, point) - shape.facing + Math.PI * 3) % (Math.PI * 2)) - Math.PI,
    );
    return delta <= shape.spread / 2;
  }
  // Линия: проекция точки на отрезок.
  const dx = Math.cos(shape.facing);
  const dy = Math.sin(shape.facing);
  const along = Math.max(
    0,
    Math.min(shape.len, (point.x - shape.x) * dx + (point.y - shape.y) * dy),
  );
  const near = { x: shape.x + dx * along, y: shape.y + dy * along };
  return len(sub(point, near)) <= shape.half + r;
}

/** Не выпускаем за края арены: за спиной стена, и это к лучшему. */
export function clampToArena(at: Vec, r: number): void {
  const away = sub(at, ARENA);
  const distance = len(away);
  const limit = ARENA.r - r;
  if (distance <= limit) return;
  at.x = ARENA.x + (away.x / distance) * limit;
  at.y = ARENA.y + (away.y / distance) * limit;
}
