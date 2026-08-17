# -*- coding: utf-8 -*-
"""生成 PC 端应用图标 app_icon.ico（紫底白字 RCH，与 Android 图标同风格）。

Android 参考（mipmap-xxxhdpi/ic_launcher.png 采样）：
  - 背景：紫色对角渐变，左上约 (88,67,230) → 右下约 (121,58,235)
  - 前景：白色粗体字母 "RCH"

文字居中采用「先画再按实际白色像素 bbox 取中」：Pillow textbbox 对 Arial Black
这类字体返回的字面度量含过大侧边距（宽度甚至超过画布），直接用它推 x 会画到
边界外。改为先画到临时层，取白色像素的实际范围居中粘贴，保证字母稳居图标中央
并有均匀边距。

用法：python build_artifacts/make_app_icon.py
"""
import os
from PIL import Image, ImageDraw, ImageFont

SIZE = 1024
C1 = (88, 67, 230)   # 左上紫
C2 = (121, 58, 235)  # 右下紫
FONT_PATH = r"C:\Windows\Fonts\ariblk.ttf"
OUT = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
                   "app", "windows", "runner", "resources", "app_icon.ico")
# 字母相对画布的横向占比（留出四周边距，避免字母贴边）。
# 默认 0.62：比占满全宽明显缩小、四边留有清楚边距，又不至于太小。
LETTERS_RATIO = 0.62


def diag_gradient(w, h, c1, c2):
    """对角线渐变：左上 c1 → 右下 c2（t = (x/w + y/h)/2）。"""
    img = Image.new("RGB", (w, h))
    px = img.load()
    for y in range(h):
        ty = y / (h - 1)
        for x in range(w):
            t = (x / (w - 1) + ty) / 2
            px[x, y] = tuple(int(c1[i] + (c2[i] - c1[i]) * t) for i in range(3))
    return img


def text_pixel_bbox(layer):
    """返回不透明白色像素的 (min_x, min_y, max_x, max_y)，无像素则 None。"""
    w, h = layer.size
    px = layer.load()
    min_x = min_y = w
    max_x = max_y = -1
    for y in range(h):
        for x in range(w):
            r, g, b, a = px[x, y]
            if a > 200 and r > 200 and g > 200 and b > 200:
                if x < min_x:
                    min_x = x
                if x > max_x:
                    max_x = x
                if y < min_y:
                    min_y = y
                if y > max_y:
                    max_y = y
    if max_x < 0:
        return None
    return (min_x, min_y, max_x, max_y)


def main():
    # 1) 渐变圆角底
    canvas = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    grad = diag_gradient(SIZE, SIZE, C1, C2).convert("RGBA")
    mask = Image.new("L", (SIZE, SIZE), 0)
    ImageDraw.Draw(mask).rounded_rectangle(
        [0, 0, SIZE - 1, SIZE - 1], radius=180, fill=255)
    canvas.paste(grad, (0, 0), mask)

    # 2) 先按估算字号画纯白 RCH 到临时层
    trial_font = ImageFont.truetype(FONT_PATH, 460)
    d0 = ImageDraw.Draw(canvas)
    d0.text((0, 0), "RCH", font=trial_font, fill=(255, 255, 255, 255))
    bbox = text_pixel_bbox(canvas)
    cur_w = bbox[2] - bbox[0] + 1

    # 3) 清掉草稿，按目标占比调整字号后正式绘制
    canvas.paste(grad, (0, 0), mask)
    target_w = int(SIZE * LETTERS_RATIO)
    font_size = max(80, round(460 * target_w / cur_w))
    font = ImageFont.truetype(FONT_PATH, font_size)
    d = ImageDraw.Draw(canvas)
    d.text((0, 0), "RCH", font=font, fill=(255, 255, 255, 255))

    # 4) 按实际白色像素范围水平垂直居中
    fb = text_pixel_bbox(canvas)
    bw, bh = fb[2] - fb[0] + 1, fb[3] - fb[1] + 1
    dx, dy = (SIZE - bw) // 2 - fb[0], (SIZE - bh) // 2 - fb[1]
    canvas2 = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    canvas2.paste(grad, (0, 0), mask)
    canvas2.alpha_composite(canvas, (dx, dy))
    canvas = canvas2

    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    canvas.save(OUT, format="ICO",
                sizes=[(16, 16), (24, 24), (32, 32), (48, 48),
                       (64, 64), (128, 128), (256, 256)])
    print("written:", OUT)
    print(f"final letter bbox: {fb}, size {bw}x{bh}, font_size={font_size}")


if __name__ == "__main__":
    main()
