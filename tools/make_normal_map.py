"""Rebuild src/textures/earth_normal.jpg from NASA's GEBCO elevation grid.

    python make_normal_map.py   (needs numpy, scipy, pillow; ~1 GB RAM)

Convention, which the globe shader relies on: R = east, G = north, B = up,
each mapped from [-1, 1] to [0, 255]. Get this backwards and every mountain
range is lit from the wrong side.
"""
import urllib.request
import numpy as np
from PIL import Image
from scipy.ndimage import gaussian_filter

SRC = "https://eoimages.gsfc.nasa.gov/images/imagerecords/73000/73934/gebco_08_rev_elev_21600x10800.png"
Image.MAX_IMAGE_PIXELS = None

urllib.request.urlretrieve(SRC, "gebco.png")
h = np.asarray(Image.open("gebco.png").convert("L").resize((8192, 4096), Image.LANCZOS)).astype(np.float32) / 255
h = gaussian_filter(h, 0.7)

lat = np.linspace(90, -90, h.shape[0])[:, None]
coslat = np.maximum(np.cos(np.radians(lat)), 0.05)
dx = (np.roll(h, -1, 1) - np.roll(h, 1, 1)) * 0.5 / coslat   # rise toward the east
dy = (np.roll(h, -1, 0) - np.roll(h, 1, 0)) * 0.5            # rise toward the south

STRENGTH = 38.0
east, north, up = -dx * STRENGTH, dy * STRENGTH, np.ones_like(h)
norm = np.sqrt(east**2 + north**2 + up**2)
rgb = ((np.stack([east, north, up], -1) / norm[..., None] * 0.5 + 0.5) * 255).clip(0, 255).astype(np.uint8)
Image.fromarray(rgb).save("../src/textures/earth_normal.jpg", quality=90, optimize=True)
print("wrote ../src/textures/earth_normal.jpg")
