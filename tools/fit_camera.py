"""How the default wallpaper camera was chosen.

The reference is a tablet screenshot of the system Earth wallpaper. Five
coastline landmarks were read off it by eye and the limb was sampled at a
dozen points; the camera below is the least-squares solution that puts the
landmarks and the limb in the same place.

    landmark        lon, lat           reference pixel (1441 x 1080)
    Kyushu          130.70, 32.60      (635, 752)
    Shikoku         133.50, 33.70      (688, 732)
    Osaka           135.50, 34.69      (712, 722)
    Busan           129.05, 35.15      (612, 705)
    Taiwan          121.00, 23.70      (240, 994)
    limb            circle through (0,740) ... (780,464) ... (1440,820)

Result (landmark rms 16 px, limb rms 1.1% of the radius):
    camera above 19.00 N, 135.69 E, 1.42 earth radii from the centre,
    tilted 23.5 deg toward the horizon, heading 5.6 deg,
    focal length 1.747 half-heights, optical axis 0.546 half-heights below
    the screen centre and 0.03 left of it.

Camera model (identical in earth.js): look straight down with north up,
right = eye x north; turn by `heading` about the vertical, then pitch up by
`tilt` about the right axis; pinhole projection with square pixels.

2026-09-22 REFIT (current defaults). The reference above misread Hainan as
Taiwan and put the camera half again too close. Refit against the user's own
iPad home screen (2000 x 1499), twelve landmarks plus fourteen limb points:

    Cape Sata 130.66,31.0 (887,1078)   Kanmon 130.95,33.95 (912,1010)
    Shikoku 133.5,33.7 (960,1015)      Osaka 135.45,34.65 (1000,1000)
    Kii tip 135.77,33.45 (1007,1027)   Tokyo Bay 139.8,35.45 (1090,990)
    Noto 137.3,37.5 (1065,935)         Hainan 109.7,19.2 (335,1385)
    Hong Kong 114.2,22.3 (440,1295)    Taiwan N 121.6,25.3 (640,1220)
    Hangzhou Bay 121.5,30.4 (650,1090) Busan 129.05,35.1 (855,1000)
    limb y at x=300..1900: 846 787 740 702 673 637 641 674 702 739 786 846 923 1013

Result (landmark rms 6 px, limb rms 0.06% of the radius):
    camera above 0.843 N, home longitude -0.106 deg, 3.213 earth radii,
    no tilt, heading 0.186 deg, focal length 5.166 half-heights,
    optical axis 1.541 half-heights below the screen centre (off-screen),
    0.0006 left. I.e. a long lens aimed at the northern limb from above the
    equator -- Apple's framing. config.rs upgrades configs still holding the
    old fitted values.
"""
