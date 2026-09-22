"""Bake src/textures/earth_day_apple.jpg: the Blue Marble day map graded toward
Apple's Earth wallpaper / Maps satellite look.

    python make_day_apple.py   (needs numpy, scipy, pillow; ~6 GB RAM)
Run make_night_water_ocean.py first (it downloads gebco.png and writes the
8K water mask this script reads).

Land: +4% saturation, arid ground pushed toward ochre, a little yellow in the
greens, local contrast ("clarity"), and ambient occlusion from GEBCO elevation
so valleys stay dark whatever the sun does. Ocean: the topo-bathy depth
shading remapped onto a muted deep-navy -> teal-shelf ramp, keeping some
sea-floor structure.
"""
T="../src/textures/"
from PIL import Image; import numpy as np
from scipy import ndimage as ndi
Image.MAX_IMAGE_PIXELS=None
W,H=8192,4096
d=np.asarray(Image.open(T+'earth_day.jpg').convert('RGB')).astype(np.float32)/255
water=np.asarray(Image.open(T+'earth_water.jpg').convert('L')).astype(np.float32)/255; w=water[...,None]
lin=d**2.2
def blur(x,s):
    if x.ndim==3: return np.stack([ndi.gaussian_filter(x[...,i],s,mode=('nearest','wrap')) for i in range(3)],-1)
    return ndi.gaussian_filter(x,s,mode=('nearest','wrap'))
def ss(a,b,x):
    t=np.clip((x-a)/(b-a),0,1); return t*t*(3-2*t)
# ---------------- land ----------------
luma=lin@np.array([0.2126,0.7152,0.0722],np.float32)
# saturation boost (luma preserving, linear light)
land=luma[...,None]+(lin-luma[...,None])*1.04
land=np.clip(land,0,None)
# hue-targeted: warm the arid ground, lift yellow into the greens
r,g,b=d[...,0],d[...,1],d[...,2]
arid=ss(0.02,0.10,r-b)*ss(0.25,0.55,r)          # bright, red>blue: desert/steppe
veg=ss(0.0,0.05,g-r)*ss(0.0,0.06,g-b)          # green dominant
lA=land@np.array([0.2126,0.7152,0.0722],np.float32)
land=land+(land-lA[...,None])*(arid[...,None]*0.18)            # arid ground: richer ochre
land=land*(1+arid[...,None]*np.array([0.11,-0.08,-0.10],np.float32))
land=land*(1-arid[...,None]*0.18)
land=land*(1+veg[...,None]*np.array([0.06,0.02,-0.04],np.float32))
# local contrast ("clarity") on luminance
L=land@np.array([0.2126,0.7152,0.0722],np.float32)
Lb=blur(L,20)
ratio=np.clip((L+0.004)/(Lb+0.004),0.3,3.0)**0.35
land=land*ratio[...,None]
# fine detail: small-radius unsharp mask on luminance, so the 8K map holds up on a 4K screen
L2=land@np.array([0.2126,0.7152,0.0722],np.float32)
fine=np.clip((L2+0.003)/(blur(L2,1.3)+0.003),0.5,2.0)**0.6
land=land*fine[...,None]
# ambient occlusion from GEBCO elevation: valleys darker, sun-independent
g8=np.asarray(Image.open('gebco.png').convert('L').resize((W,H),Image.LANCZOS)).astype(np.float32)/255
ao=np.zeros_like(g8)
for s,k in [(2,6.0),(6,3.0),(16,1.6)]:
    ao+=np.clip((blur(g8,s)-g8)*k,0,1)
ao=np.clip(1-ao*0.9,0.45,1)
land=land*ao[...,None]
# ---------------- ocean ----------------
lum=lin.mean(2)
lf=blur(lum,1.2); lc=blur(lum,6)
wm=water>0.5
def norm(x):
    lo,hi=np.percentile(x[wm],[3,99.5]); return np.clip((np.log(x+1e-4)-np.log(lo+1e-4))/(np.log(hi+1e-4)-np.log(lo+1e-4)),0,1)
t=(0.65*norm(lc)+0.35*norm(lf))**1.5          # 0 deep .. 1 shelf
def srgb(c): return (np.array(c,np.float32)/255)**2.2
deep,mid,shelf=srgb((22,38,62)),srgb((30,56,86)),srgb((52,104,112))
t3=t[...,None]
oc=np.where(t3<0.55, deep+(mid-deep)*(t3/0.55), mid+(shelf-mid)*((t3-0.55)/0.45))
# the shallowest shelves (Yellow Sea, Bahamas, Gulf of Thailand) go turquoise, as in the reference
lagoon=srgb((80,140,138))
oc=oc+(lagoon-oc)*(ss(0.80,1.0,t3)*0.7)
out=np.clip(land*(1-w)+oc*w,0,1)**(1/2.2)
Image.fromarray((out*255+0.5).astype(np.uint8)).save(T+'earth_day_apple.jpg',quality=90)
print('wrote earth_day_apple.jpg')
