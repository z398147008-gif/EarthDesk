"""EarthDesk icon, iOS 27 Liquid-Glass style.

Layers (back to front):
  1. squircle plate  - deep-space gradient (or light sky for variant B), faint stars
  2. earth           - real textures, evening terminator over Japan, amber city lights,
                       clouds, ocean glint, atmosphere limb, soft cast shadow on plate
  3. glass           - static crisp edge highlights top + bottom on globe and plate,
                       thin dark hairline between layers (iOS 27 "defined layers")
  4. (variant C)     - frosted glass widget card refracting the globe
"""
import sys, numpy as np
from PIL import Image, ImageFilter
from scipy.ndimage import map_coordinates, gaussian_filter

# Usage (from repo root):  python tools/make_icon.py   -> writes src-tauri/icons/*
# Needs numpy, scipy, pillow. Variant A is the shipped icon; B (light sky) and C (glass
# widget card) were the rejected alternatives, kept for reference.
import os
ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..')
Image.MAX_IMAGE_PIXELS = None
SS = 2048            # render size, downsampled to 1024
def T(n):
    im = Image.open(os.path.join(ROOT, 'src', 'textures', n + '.jpg'))
    im = im.convert('L' if n in ('earth_clouds', 'earth_water') else 'RGB').resize((4096, 2048), Image.LANCZOS)
    return np.asarray(im, dtype=np.float32) / 255.0
DAY, NIGHT, CLOUD, WATER = T('earth_day_apple'), T('earth_night'), T('earth_clouds'), T('earth_water')
TH, TW = DAY.shape[:2]

def smooth(a, b, x):
    t = np.clip((x - a) / (b - a), 0, 1); return t * t * (3 - 2 * t)

def srgb2lin(c): return np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)
def lin2srgb(c):
    c = np.clip(c, 0, None); return np.where(c <= 0.0031308, c * 12.92, 1.055 * c ** (1 / 2.4) - 0.055)

def sample(tex, lon, lat):
    u = ((lon / (2 * np.pi) + 0.5) % 1.0) * TW - 0.5
    v = (0.5 - lat / np.pi) * TH - 0.5
    if tex.ndim == 2:
        return map_coordinates(tex, [v, u], order=1, mode='wrap')
    return np.stack([map_coordinates(tex[..., c], [v, u], order=1, mode='wrap') for c in range(tex.shape[2])], -1)

def over(dst, rgb, a):
    a = a[..., None]; return dst * (1 - a) + rgb * a

yy, xx = (np.mgrid[0:SS, 0:SS] + 0.5) / SS          # 0..1, y down

# ---------- squircle plate ----------
BODY = 0.86                                        # plate size / canvas
def squircle_sdf_mask(scale=BODY, n=5.0):
    x = (xx - 0.5) / (scale / 2); y = (yy - 0.5) / (scale / 2)
    r = (np.abs(x) ** n + np.abs(y) ** n) ** (1 / n)
    aa = 1.5 / (SS * scale / 2)
    return np.clip((1 - r) / aa + 0.5, 0, 1), r

def render(variant='A', center_lon=112.0, center_lat=22.0, small=False):
    global BODY
    BODY = 0.95 if small else 0.86
    plate, pr = squircle_sdf_mask(BODY)
    img = np.zeros((SS, SS, 3), np.float32)

    # plate background (linear light)
    t = np.clip((yy - 0.07) / 0.86, 0, 1)
    if variant == 'B':
        top, bot = srgb2lin(np.array([0.36, 0.62, 0.95])), srgb2lin(np.array([0.10, 0.27, 0.62]))
    else:
        top, bot = srgb2lin(np.array([0.075, 0.13, 0.30])), srgb2lin(np.array([0.008, 0.012, 0.035]))
    bg = top * (1 - t[..., None]) + bot * t[..., None]
    # soft glow behind the globe from the sun side (upper left)
    g = np.exp(-(((xx - 0.30) ** 2 + (yy - 0.26) ** 2) / 0.05))
    bg += (np.array([0.10, 0.20, 0.45]) if variant != 'B' else np.array([0.35, 0.45, 0.55])) * g[..., None] * 0.35
    if variant != 'B' and not small:
        rng = np.random.default_rng(7)
        n = 170
        sx, sy = rng.random(n), rng.random(n)
        sb = rng.random(n) ** 3
        stars = np.zeros((SS, SS), np.float32)
        ix, iy = (sx * SS).astype(int), (sy * SS).astype(int)
        stars[iy, ix] = sb * 6.0
        stars = gaussian_filter(stars, 1.3)
        bg += stars[..., None] * np.array([0.9, 0.95, 1.0])
    img = bg.copy()

    # ---------- earth ----------
    CX, CY, R = (0.5, 0.5, 0.385) if small else (0.5, 0.515, 0.315)
    x = (xx - CX) / R; y = -(yy - CY) / R
    rr = np.sqrt(x * x + y * y)
    inside = rr < 1
    z = np.sqrt(np.clip(1 - rr * rr, 0, 1))
    # view -> world: rotate by lat (about x) then lon (about y)
    la, lo = np.radians(center_lat), np.radians(center_lon)
    # view basis: forward (out of screen) = world point at (lo, la)
    fwd = np.array([np.cos(la) * np.sin(lo), np.sin(la), np.cos(la) * np.cos(lo)])
    east = np.array([np.cos(lo), 0, -np.sin(lo)])
    north = np.cross(fwd, east)
    P = x[..., None] * east + y[..., None] * north + z[..., None] * fwd
    lat = np.arcsin(np.clip(P[..., 1], -1, 1))
    lon = np.arctan2(P[..., 0], P[..., 2])

    m = inside
    day = srgb2lin(sample(DAY, lon[m], lat[m]))
    night = srgb2lin(sample(NIGHT, lon[m], lat[m]))
    cloud = sample(CLOUD, lon[m], lat[m])
    water = sample(WATER, lon[m], lat[m])

    sun = np.array([-0.93, 0.30, 0.20]); sun /= np.linalg.norm(sun)   # view space
    N = np.stack([x[m], y[m], z[m]], -1)
    ndl = N @ sun
    lit = smooth(-0.07, 0.20, ndl)
    diff = np.clip(ndl * 0.85 + 0.15, 0, 1)

    cl = smooth(0.30, 0.85, cloud) * 0.85
    surf = day * (0.03 + 1.25 * diff[:, None] * lit[:, None])
    # ocean glint
    H = sun + np.array([0, 0, 1.0]); H /= np.linalg.norm(H)
    ndh = np.clip(N @ H, 0, 1)
    glint = (ndh ** 70 * 0.35 + ndh ** 600 * 0.9) * water * (1 - cl) * lit
    surf += glint[:, None] * np.array([1.0, 0.92, 0.80])
    # city lights on the night side (amber, point-like)
    L = np.clip(night.mean(-1) * 1.0, 0, None)
    L = L ** 1.15 * 4.0
    city = L[:, None] * np.array([1.30, 0.72, 0.26]) * (1 - lit)[:, None] * (1 - cl * 0.8)[:, None]
    surf += city
    # moonlit night floor
    surf += day * 0.025 * (1 - lit)[:, None]
    # clouds
    cc = np.array([1.0, 1.0, 1.0]) * (0.012 + 1.05 * diff * lit)[:, None]
    surf = surf * (1 - cl[:, None]) + cc * cl[:, None]
    # atmosphere: limb brightening, blue, stronger on lit side; warm band at terminator
    zz = z[m]
    limb = (1 - zz) ** 2.2
    atm_lit = smooth(-0.35, 0.45, ndl)
    surf = surf * (1 - limb[:, None] * 0.55 * atm_lit[:, None]) + \
        np.array([0.30, 0.55, 1.10]) * (limb * 0.75 * atm_lit)[:, None]
    term = np.exp(-((ndl - 0.02) / 0.10) ** 2) * limb * 0.9
    surf += np.array([0.9, 0.45, 0.18]) * term[:, None] * 0.12

    earth = np.zeros((SS, SS, 3), np.float32); earth[m] = surf
    aa = 1.5 / (SS * R)
    emask = np.clip((1 - rr) / aa + 0.5, 0, 1)

    # cast shadow of the globe layer onto the plate (layer depth)
    sh = gaussian_filter(((xx - CX) ** 2 + (yy - CY - 0.035) ** 2 < (R * 0.98) ** 2).astype(np.float32), SS * 0.03)
    img *= (1 - 0.55 * sh)[..., None]
    # outer atmosphere halo on plate
    halo_r = np.clip(rr - 1, 0, None)
    halo = np.exp(-halo_r / 0.045) * (rr >= 1)
    hdir = np.clip((x * sun[0] + y * sun[1]) / np.maximum(rr, 1e-6) * 0.7 + 0.45, 0, 1)
    img += np.array([0.20, 0.42, 1.0])[None, None] * (halo * hdir * 0.55)[..., None]
    # dark hairline separating layers (iOS 27)
    hair = np.exp(-((rr - 1.0) / (2.2 / (SS * R))) ** 2) * 0.35
    img *= (1 - hair)[..., None]
    img = over(img, earth, emask)

    # glass rim on globe: crisp static highlights top and bottom
    ang = np.arctan2(y, x)                            # up = +pi/2
    rim_band = np.exp(-((rr - (1 - 3.0 / (SS * R))) / (2.0 / (SS * R))) ** 2) * inside
    top = np.clip(np.sin(ang), 0, 1) ** 3
    bottom = np.clip(-np.sin(ang), 0, 1) ** 4
    img += (rim_band * (top * 0.85 + bottom * 0.30))[..., None] * np.array([1.0, 1.0, 1.0])
    # broad inner glass sheen (very soft) at upper-left
    sheen = np.exp(-(((x + 0.35) ** 2) / 0.10 + ((y - 0.55) ** 2) / 0.03)) * inside * 0.06
    img += sheen[..., None]

    # ---------- variant C: frosted glass widget card ----------
    if variant == 'C' and not small:
        cx0, cy0, cx1, cy1 = 0.49, 0.60, 0.86, 0.83
        cw, ch = cx1 - cx0, cy1 - cy0
        qx = np.abs(xx - (cx0 + cx1) / 2) - (cw / 2 - 0.05)
        qy = np.abs(yy - (cy0 + cy1) / 2) - (ch / 2 - 0.05)
        d = np.sqrt(np.clip(qx, 0, None) ** 2 + np.clip(qy, 0, None) ** 2) + np.minimum(np.maximum(qx, qy), 0) - 0.05
        card = np.clip(-d * SS / 1.5 + 0.5, 0, 1)
        # refraction: sample blurred content shifted toward card center near edges
        blur = np.stack([gaussian_filter(img[..., c], SS * 0.006) for c in range(3)], -1)
        edge = np.exp(-np.clip(-d, 0, None) / 0.012)
        ox = (xx - (cx0 + cx1) / 2); oy = (yy - (cy0 + cy1) / 2)
        sxp = np.clip(((xx - ox * edge * 0.25) * SS).astype(int), 0, SS - 1)
        syp = np.clip(((yy - oy * edge * 0.25) * SS).astype(int), 0, SS - 1)
        refr = blur[syp, sxp]
        vgrad = np.clip(1 - (yy - cy0) / ch, 0, 1)[..., None]
        glass = refr * 1.35 + np.array([0.75, 0.85, 1.0]) * (0.012 + 0.03 * vgrad)
        # card shadow
        csh = gaussian_filter(card, SS * 0.02); csh = np.roll(csh, int(SS * 0.018), 0)
        img *= (1 - 0.45 * csh * (1 - card))[..., None]
        img = over(img, glass, card)
        # edge highlights top/bottom of card
        ring = np.exp(-((d + 2.5 / SS) / (1.6 / SS)) ** 2)
        img += (ring * (np.clip(-oy / (ch / 2), 0, 1) ** 2 * 0.8 + np.clip(oy / (ch / 2), 0, 1) ** 2 * 0.25))[..., None]
        img *= (1 - 0.3 * np.exp(-((d - 1.0 / SS) / (1.5 / SS)) ** 2))[..., None]
        # glyphs: sun disc + three bars (weather + perf), white, simple
        gx, gy, gr = cx0 + 0.080, (cy0 + cy1) / 2, 0.040
        sund = np.sqrt((xx - gx) ** 2 + (yy - gy) ** 2)
        g1 = np.clip((gr - sund) * SS / 1.5 + 0.5, 0, 1)
        img = over(img, srgb2lin(np.array([1.0, 0.84, 0.36])), g1 * card)
        for i, h in enumerate([0.55, 0.85, 0.40]):
            bx = cx0 + 0.185 + i * 0.050
            bh = (ch - 0.08) * h
            by1 = cy1 - 0.045
            qx2 = np.abs(xx - bx) - 0.0
            qy2 = np.abs(yy - (by1 - bh / 2)) - (bh / 2)
            d2 = np.sqrt(np.clip(qx2, 0, None) ** 2 + np.clip(qy2, 0, None) ** 2) + np.minimum(np.maximum(qx2, qy2), 0) - 0.010
            b = np.clip(-d2 * SS / 1.5 + 0.5, 0, 1)
            img = over(img, np.array([0.92, 0.95, 1.0]), b * card * 0.95)

    # ---------- plate glass edge ----------
    pw = 1.0 / (SS * BODY / 2)
    prim = np.exp(-((pr - (1 - 3.5 * pw)) / (2.2 * pw)) ** 2)
    py = (yy - 0.5) / (BODY / 2)
    img += (prim * (np.clip(-py, 0, 1) ** 2.5 * 0.55 + np.clip(py, 0, 1) ** 3 * 0.22))[..., None]

    out = lin2srgb(img)
    # gentle filmic shoulder
    lum = out.max(-1, keepdims=True)
    out = out * (1 / (1 + np.clip(lum - 0.85, 0, None) * 1.2))
    rgba = np.concatenate([np.clip(out, 0, 1), plate[..., None]], -1)
    im = Image.fromarray((rgba * 255 + 0.5).astype(np.uint8), 'RGBA')

    # soft drop shadow under the plate on transparent canvas
    shadow = gaussian_filter(np.roll(plate, int(SS * 0.012), 0), SS * 0.012) * 0.45
    base = np.zeros((SS, SS, 4), np.float32); base[..., 3] = shadow
    bim = Image.fromarray((base * 255).astype(np.uint8), 'RGBA')
    bim.alpha_composite(im)
    return bim.resize((1024, 1024), Image.LANCZOS)

if __name__ == '__main__':
    from PIL.ImageFilter import UnsharpMask
    v = (sys.argv[1:] or ['A'])[0]
    big, small = render(v), render(v, small=True)   # small: bigger plate + globe, no stars/card
    def at(s):
        im = (small if s <= 32 else big).resize((s, s), Image.LANCZOS)
        return im.filter(UnsharpMask(radius=0.6, percent=60, threshold=0)) if s <= 48 else im
    out = os.path.join(ROOT, 'src-tauri', 'icons')
    big.save(os.path.join(out, 'icon-source.png'))
    for s in [16, 32, 48, 64, 128, 256]:
        at(s).save(os.path.join(out, f'{s}x{s}.png'))
    sizes = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256]
    imgs = [at(s) for s in sizes]
    imgs[-1].save(os.path.join(out, 'icon.ico'), format='ICO', sizes=[(s, s) for s in sizes], append_images=imgs[:-1])
    print('icons written to', out)
