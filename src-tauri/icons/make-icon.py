#!/usr/bin/env python3
"""Генератор исходной иконки БРЕД.

Иконка держится тех же правил, что и интерфейс: монохром, никаких градиентов,
смысл — терминальная строка. Буква «Б» и мигающий курсор рядом.

Рисуем аналитически со сглаживанием (супервыборка 4×4), а не по пикселям:
именно из-за отсутствия сглаживания предыдущая версия выглядела рвано.
"""

import struct
import zlib

import numpy as np

SIZE = 1024
SUPER = 4  # выборок на пиксель по каждой оси

BG = 0x0D  # почти чёрный, как фон приложения
FG = 0xF2  # почти белый, как текст
EDGE = 0x2E  # тонкая рамка, чтобы иконка не сливалась с тёмным доком


def grid():
    """Координатная сетка в супервыборке, в единицах итогового изображения."""
    step = 1.0 / SUPER
    axis = np.arange(SIZE * SUPER, dtype=np.float32) * step + step / 2
    return np.meshgrid(axis, axis)  # (x, y)


def squircle(x, y, cx, cy, half, power=4.6):
    """Скруглённый квадрат в стиле macOS — суперэллипс, а не радиус."""
    dx = np.abs(x - cx) / half
    dy = np.abs(y - cy) / half
    return dx**power + dy**power <= 1.0


def rounded_rect(x, y, x0, y0, x1, y1, r=0.0):
    """Прямоугольник со скруглением всех углов."""
    if r <= 0:
        return (x >= x0) & (x <= x1) & (y >= y0) & (y <= y1)

    inner_x = np.clip(x, x0 + r, x1 - r)
    inner_y = np.clip(y, y0 + r, y1 - r)
    near = ((x - inner_x) ** 2 + (y - inner_y) ** 2) <= r * r
    return (x >= x0) & (x <= x1) & (y >= y0) & (y <= y1) & near


def right_rounded(x, y, x0, y0, x1, y1, r):
    """Скругление только справа — так строится чаша буквы «Б»."""
    inner_x = np.minimum(x, x1 - r)
    inner_y = np.clip(y, y0 + r, y1 - r)
    near = ((x - np.maximum(inner_x, x0)) ** 2 + (y - inner_y) ** 2) <= r * r
    return (x >= x0) & (x <= x1) & (y >= y0) & (y <= y1) & (near | (x <= x1 - r))


def letter_be(x, y):
    """Буква «Б»: стойка, верхняя перекладина и чаша."""
    stem_w = 82
    left, top, bottom = 258, 286, 742
    bar_right = 566
    bowl_top = 470
    bowl_right = 648
    bowl_r = 128

    stem = rounded_rect(x, y, left, top, left + stem_w, bottom, r=6)
    bar = rounded_rect(x, y, left, top, bar_right, top + stem_w, r=6)

    bowl_outer = right_rounded(x, y, left, bowl_top, bowl_right, bottom, bowl_r)
    bowl_inner = right_rounded(
        x,
        y,
        left + stem_w,
        bowl_top + stem_w,
        bowl_right - stem_w,
        bottom - stem_w,
        max(bowl_r - stem_w, 12),
    )
    return stem | bar | (bowl_outer & ~bowl_inner)


def cursor(x, y):
    """Курсор терминала — сплошной блок правее буквы.

    По высоте выровнен с чашей «Б»: более низкий блок читался как точка.
    """
    return rounded_rect(x, y, 706, 470, 806, 742, r=8)


def render():
    x, y = grid()

    plate = squircle(x, y, SIZE / 2, SIZE / 2, SIZE / 2 - 26)
    inner = squircle(x, y, SIZE / 2, SIZE / 2, SIZE / 2 - 34)
    border = plate & ~inner

    glyph = (letter_be(x, y) | cursor(x, y)) & inner

    # Собираем в оттенках серого, затем усредняем супервыборку.
    grey = np.full(plate.shape, 0, dtype=np.float32)
    grey[inner] = BG
    grey[border] = EDGE
    grey[glyph] = FG

    alpha = np.zeros(plate.shape, dtype=np.float32)
    alpha[plate] = 255.0

    def downsample(source):
        return (
            source.reshape(SIZE, SUPER, SIZE, SUPER).mean(axis=(1, 3)).astype(np.uint8)
        )

    return downsample(grey), downsample(alpha)


def write_png(path, grey, alpha):
    rgba = np.dstack([grey, grey, grey, alpha]).astype(np.uint8)
    raw = b"".join(b"\x00" + rgba[row].tobytes() for row in range(SIZE))

    def chunk(tag, data):
        return (
            struct.pack(">I", len(data))
            + tag
            + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", SIZE, SIZE, 8, 6, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(raw, 9))
    png += chunk(b"IEND", b"")

    with open(path, "wb") as handle:
        handle.write(png)
    return len(png)


if __name__ == "__main__":
    grey, alpha = render()
    size = write_png("icons/source.png", grey, alpha)
    print(f"icons/source.png — {SIZE}×{SIZE}, {size // 1024} КБ")
