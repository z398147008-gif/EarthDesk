"""Bake src/textures/earth_water.jpg (8192x4096, white = water) from Natural
Earth 10m coastlines, lakes and minor islands.

    python make_water_mask.py   (needs numpy, pillow, pyshp; ~1 GB RAM)

The old mask was GEBCO elevation == 0. That PNG is 8-bit, one step is about
25 m, so every plain lower than that counted as sea: the North China Plain,
the Yangtze delta, Jiangsu, the Liao plain -- "normal land drowned by the
sea". Natural Earth draws the real coastline instead.

Rasterised at 2x and box-filtered down, so the coast is antialiased the way
the shader expects (it blends land and sea by this value).
Run from tools/; downloads the shapefiles on first run (~6 MB).
"""
import os, io, zipfile, urllib.request
import numpy as np
from PIL import Image, ImageDraw
import shapefile

Image.MAX_IMAGE_PIXELS = None
W, H = 8192, 4096
SS = 2
T = "../src/textures/"
URL = "https://naciscdn.org/naturalearth/10m/physical/{}.zip"


def shapes(name):
    if not os.path.exists(name + ".zip"):
        urllib.request.urlretrieve(URL.format(name), name + ".zip")
    z = zipfile.ZipFile(name + ".zip")
    r = shapefile.Reader(shp=io.BytesIO(z.read(name + ".shp")),
                         shx=io.BytesIO(z.read(name + ".shx")),
                         dbf=io.BytesIO(z.read(name + ".dbf")))
    return r.shapes()


def rings(shape):
    pts = shape.points
    idx = list(shape.parts) + [len(pts)]
    for a, b in zip(idx[:-1], idx[1:]):
        ring = pts[a:b]
        # Shapefile convention: outer rings clockwise, holes counter-clockwise.
        area = sum(x0 * y1 - x1 * y0 for (x0, y0), (x1, y1) in zip(ring, ring[1:] + ring[:1]))
        yield ring, area < 0


def xy(ring):
    return [((lon + 180.0) / 360.0 * W * SS, (90.0 - lat) / 180.0 * H * SS) for lon, lat in ring]


img = Image.new("L", (W * SS, H * SS), 255)     # start as water
d = ImageDraw.Draw(img)


def fill(name, outer, hole):
    for s in shapes(name):
        for ring, is_outer in rings(s):
            if len(ring) >= 3:
                d.polygon(xy(ring), fill=outer if is_outer else hole)


fill("ne_10m_land", 0, 255)
fill("ne_10m_minor_islands", 0, 255)
fill("ne_10m_lakes", 255, 0)

water = img.resize((W, H), Image.BOX)
water.save(T + "earth_water.jpg", quality=92)
a = np.asarray(water)
print("wrote earth_water.jpg, water fraction %.3f" % (a.mean() / 255))
