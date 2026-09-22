"""Rebuild three globe textures from NASA sources.

    python make_night_water_ocean.py   (needs numpy, scipy, pillow; ~4 GB RAM)

  earth_night.jpg     NASA Black Marble 2016 (3 km), 8192x4096. Downsampled in
                      linear light, then the moonlit ground is stripped: only
                      warm (artificial) light survives, so the shader needs no
                      floor and the night side is black where nobody lives.
  earth_water.jpg     8192x4096 sea mask from GEBCO (elevation == 0), plus
                      inland lakes taken from the old 2048 mask away from coasts.
  earth_day_calm.jpg  earth_day.jpg with the sea-floor relief removed: deep
                      ocean an even navy, only continental shelves lighter.
Run from tools/; reads and writes ../src/textures/.
"""
import os, urllib.request
import numpy as np
from PIL import Image
from scipy import ndimage as ndi

Image.MAX_IMAGE_PIXELS = None
W, H = 8192, 4096
T = "../src/textures/"
SRC = {
    "gebco.png": "https://eoimages.gsfc.nasa.gov/images/imagerecords/73000/73934/gebco_08_rev_elev_21600x10800.png",
    "blackmarble.jpg": "https://eoimages.gsfc.nasa.gov/images/imagerecords/144000/144898/BlackMarble_2016_3km.jpg",
}
for f, u in SRC.items():
    if not os.path.exists(f):
        urllib.request.urlretrieve(u, f)

# --- water mask --------------------------------------------------------------
g = Image.open("gebco.png").convert("L")
sea = np.asarray(g.resize((W, H), Image.BOX)) == 0
seaf = np.asarray(g.point(lambda v: 255 if v == 0 else 0).resize((W, H), Image.LANCZOS)).astype(np.float32) / 255
old = np.asarray(Image.open(T + "earth_water_2048.jpg").convert("L")) > 128 if os.path.exists(T + "earth_water_2048.jpg") else None
water = seaf
if old is not None:
    old = ndi.binary_erosion(old, iterations=2)
    oldup = np.asarray(Image.fromarray(old.astype(np.uint8) * 255).resize((W, H), Image.BILINEAR)) > 128
    lake = oldup & (ndi.distance_transform_edt(~sea) > 16)
    water = np.maximum(seaf, ndi.gaussian_filter(lake.astype(np.float32), 1.0))
water = np.clip(water, 0, 1)
Image.fromarray((water * 255).astype(np.uint8)).save(T + "earth_water.jpg", quality=92)

# --- night lights ------------------------------------------------------------
a = np.asarray(Image.open("blackmarble.jpg").convert("RGB")).astype(np.float32) / 255
lin = a ** 2.2
chs = [np.asarray(Image.fromarray(lin[..., i]).resize((W, H), Image.LANCZOS)) for i in range(3)]
o = np.clip(np.stack(chs, -1), 0, 1) ** (1 / 2.2)
L = np.clip(np.minimum(o[..., 0], o[..., 1]) - 0.62 * o[..., 2], 0, 1)
t = np.clip(L / 0.22, 0, 1)
o = o * (t * t * (3 - 2 * t))[..., None]
Image.fromarray((o * 255 + 0.5).astype(np.uint8)).save(T + "earth_night.jpg", quality=92)

# --- calm ocean --------------------------------------------------------------
d = np.asarray(Image.open(T + "earth_day.jpg").convert("RGB")).astype(np.float32) / 255
lin = d ** 2.2
wm = water > 0.5
lb = ndi.gaussian_filter(lin.mean(2), 6, mode=("nearest", "wrap"))
pc = np.percentile(lb[wm], [50, 80, 90, 95, 98])
deep = np.median(lin[wm & (lb < pc[0])], 0)
shelf = np.median(lin[wm & (lb > pc[3])], 0)
x = np.clip((lb - pc[2]) / (pc[4] - pc[2]), 0, 1)
s = x * x * (3 - 2 * x) * 0.75
calm = deep + (shelf - deep) * s[..., None]
w = water[..., None]
out = np.clip(lin * (1 - w) + calm * w, 0, 1) ** (1 / 2.2)
Image.fromarray((out * 255 + 0.5).astype(np.uint8)).save(T + "earth_day_calm.jpg", quality=90)
print("wrote earth_water.jpg, earth_night.jpg, earth_day_calm.jpg")
