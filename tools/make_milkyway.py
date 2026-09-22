"""Bake src/textures/milkyway.jpg from NASA SVS Deep Star Maps 2020
(https://svs.gsfc.nasa.gov/4851, milkyway_2020_4k.exr, celestial coordinates,
bright stars removed; RA 0 at the centre, increasing to the left).

    pip install OpenEXR numpy scipy pillow; python make_milkyway.py
"""
import os, urllib.request
import numpy as np, OpenEXR
from PIL import Image
from scipy import ndimage as ndi
URL = "https://svs.gsfc.nasa.gov/vis/a000000/a004800/a004851/milkyway_2020_4k.exr"
if not os.path.exists("mw4k.exr"):
    urllib.request.urlretrieve(URL, "mw4k.exr")
a = np.asarray(OpenEXR.File("mw4k.exr").channels()["RGB"].pixels).astype(np.float32)
# soften the unresolved faint stars, which read as grain at wallpaper scale
a = np.stack([ndi.gaussian_filter(a[..., i], 2.5, mode=("nearest", "wrap")) for i in range(3)], -1)
t = np.clip(a / np.percentile(a, 99.9), 0, 1) ** 0.75
Image.fromarray((t * 255 + 0.5).astype(np.uint8)).save("../src/textures/milkyway.jpg", quality=90)
print("wrote milkyway.jpg")
