"""Re-colour the ocean by real water depth (run after make_day_apple.py).

    python make_ocean_apple.py <land-only map in> <map out>
    # e.g. python make_ocean_apple.py earth_day_land.jpg ../src/textures/earth_day_apple.jpg

Takes the LAND-graded map (what make_day_apple.py writes, with Blue Marble's
own ocean still in it) and replaces the water with a depth ramp read off
Apple's Earth wallpaper: turquoise at the coast, cyan-blue over the shelves,
a saturated blue into the abyss, plus a faint sea-floor hillshade and slow
mottling. GEBCO bathymetry (NASA Visible Earth 73963; 255 = sea level,
0 = ~8 km deep) is downloaded on first run (~35 MB).

Coastlines come from Blue Marble's own water mask and are used exactly as
they are -- an earlier version blurred land colour out over the shelf as
"sediment", which turned the Yellow Sea to mud and swallowed the coast
around Shanghai.
"""
import os, urllib.request, sys
if not os.path.exists("gebco_bath.png"):
    urllib.request.urlretrieve(
        "https://eoimages.gsfc.nasa.gov/images/imagerecords/73000/73963/"
        "gebco_08_rev_bath_21600x10800.png", "gebco_bath.png")
import numpy as np, gc, sys
from PIL import Image
from scipy import ndimage as ndi
Image.MAX_IMAGE_PIXELS=None
BASE=sys.argv[1] if len(sys.argv)>1 else 'earth_day_land.jpg'
OUT=sys.argv[2] if len(sys.argv)>2 else '../src/textures/earth_day_apple.jpg'
W,H=8192,4096
def blur(x,s): return ndi.gaussian_filter(x,s,mode=('nearest','wrap'))
def srgb(c): return (np.array(c,np.float32)/255)**2.2
bath=np.asarray(Image.open('gebco_bath.png').convert('L').resize((W,H),Image.BILINEAR)).astype(np.float32)
depth=np.maximum((255.0-bath)/255.0*8000.0,0)
depthC=blur(depth,4.0)
# Read off Apple's Earth: turquoise right at the coast, a clear cyan-blue
# shelf, then a saturated ocean blue that keeps its colour into the abyss --
# no grey, and no olive sediment smearing the coastline into the sea.
stops=[(0,(68,141,160)),(20,(52,120,152)),(60,(43,104,144)),(150,(36,92,137)),
       (400,(31,80,130)),(1000,(28,71,123)),(2500,(24,62,114)),(4500,(21,55,106)),
       (8000,(19,49,97))]
ds=np.array([x for x,_ in stops],np.float32); cs=np.array([srgb(c) for _,c in stops],np.float32)
# sea-floor relief, weighted toward the shallows
bz=blur(bath,2.0); gx=np.roll(bz,-1,1)-np.roll(bz,1,1); gy=np.roll(bz,-1,0)-np.roll(bz,1,0); del bz,bath
shade=np.clip(1.0+(-gx+gy)*0.005,0.93,1.07); del gx,gy
rw=np.clip(1.0-depth/5000.0,0.25,1.0)
shade=1+(shade-1)*rw; del rw; gc.collect()
rng=np.random.default_rng(7); mott=np.zeros((H,W),np.float32)
for sc,amp in [(160,0.55),(60,0.30),(18,0.15)]:
    n=rng.standard_normal((H//sc+2,W//sc+2)).astype(np.float32)
    mott+=np.asarray(Image.fromarray(n).resize((W,H),Image.BICUBIC))*amp
mott=blur(mott,3); mott/=max(1e-3,mott.std())
water=np.asarray(Image.open('../src/textures/earth_water.jpg').convert('L')).astype(np.float32)/255
# Only where the mask says water; the coast stays exactly where Blue Marble
# has it (the old version blurred land colour out over the shelf, which is
# what turned the Yellow Sea to mud and ate the Shanghai coastline).
# Use Blue Marble's own water mask exactly as it is: pushing it either way
# moves coastlines (a wider mask floods the Yangtze delta and the Pearl River
# delta, which is what "Shanghai looks like it is all sea" was).
base=Image.open(BASE).convert('RGB')
out=np.empty((H,W,3),np.uint8)
for k in range(3):
    land=np.asarray(base.getchannel(k).resize((W,H),Image.LANCZOS)).astype(np.float32)/255.0
    land**=2.2
    oc=np.interp(depthC,ds,cs[:,k]).astype(np.float32)
    oc*=shade
    oc*=1+[0.05,0.05,0.03][k]*mott
    c=land*(1-water)+np.clip(oc,0,1)*water
    out[...,k]=(np.clip(c,0,1)**(1/2.2)*255+0.5).astype(np.uint8)
    del land,oc,c; gc.collect()
Image.fromarray(out).save(OUT,quality=92)
print('wrote',OUT)
