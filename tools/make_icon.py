"""Builds the square app icon from assets/logo.png (a tall banner): crop to the opaque banner, scale it to the
full height of the square (like Apex's taskbar icon), centre it, and write assets/icon.png (256) + assets/icon.ico
(every size Windows asks for). Run: python tools/make_icon.py"""
from PIL import Image

src = Image.open("assets/logo.png").convert("RGBA")
src = src.crop(src.getchannel("A").point(lambda v: 255 if v > 200 else 0).getbbox())

def square(size, fill=1.0):
    h = round(size * fill)
    w = round(h * src.width / src.height)
    im = src.resize((w, h), Image.LANCZOS)
    out = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    out.paste(im, ((size - w) // 2, (size - h) // 2), im)
    return out

square(256).save("assets/icon.png")
sizes = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256]
imgs = [square(s) for s in sizes]
imgs[-1].save("assets/icon.ico", sizes=[(s, s) for s in sizes], append_images=imgs[:-1])
print("wrote assets/icon.png, assets/icon.ico")
