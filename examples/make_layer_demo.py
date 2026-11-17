#!/usr/bin/env python3
"""Create registered demo atlas and activation volumes (only stdlib Python).

Run in project root: python3 examples/make_layer_demo.py
Works with data/sample.nii.gz from make_sample.py. All sform and pixdim
bytes are copied so niv's voxel-grid check accepts the layers.
"""
import gzip
import math
import struct
from pathlib import Path

out = Path('data')
out.mkdir(exist_ok=True)
size = (96, 80, 64)
with gzip.open(out / 'sample.nii.gz', 'rb') as fp:
    header = fp.read(352)
assert len(header) == 352
assert struct.unpack_from('<3h', header, 42) == size, 'sample.nii.gz grid changed'


def write_volume(name, fn):
    with gzip.open(out / name, 'wb', compresslevel=1) as fp:
        fp.write(header)
        for z in range(size[2]):
            for y in range(size[1]):
                fp.write(struct.pack('<96f', *(fn(x, y, z) for x in range(size[0]))))
    print('Wrote', out / name)


def inside(x,y,z):
    return ((x - 48)/38)**2 + ((y - 40)/30)**2 + ((z - 32)/24)**2 < 1


def label(x,y,z):
    if not inside(x,y,z): return 0.
    if x < 48: return 51.  # ACB color
    if y < 40: return 34.  # PVH color
    return 29.           # VISp color


def activation(x,y,z):
    if not inside(x,y,z): return 0.
    return (5 * math.exp(-(((x-36)/11)**2 + ((y-40)/13)**2 + ((z-32)/12)**2))
            + 3.8 * math.exp(-(((x-64)/12)**2 + ((y-52)/10)**2 + ((z-35)/9)**2)))

write_volume('demo_atlas.nii.gz', label)
write_volume('demo_activation.nii.gz', activation)
(out / 'demo_lut.csv').write_text('Region_ID,ID_Path,Region,Abbr,General_Region,R,G,B\n'
                                   '51,/997/,Nucleus accumbens,ACB,Striatum,128,205,248\n'
                                   '34,/997/,Paraventricular hypothalamic nucleus,PVH,Hypothalamus,255,93,80\n'
                                   '29,/997/,Primary visual area,VISp,Isocortex,8,133,140\n')
print('Wrote', out / 'demo_lut.csv')
