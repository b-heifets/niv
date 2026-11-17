#!/usr/bin/env python3
"""Create an illustrative NIfTI-1 .nii.gz using only the Python standard library."""
import gzip
import math
import struct
from pathlib import Path

size = (96, 80, 64)
header = bytearray(352)
struct.pack_into('<I', header, 0, 348)  # sizeof_hdr
struct.pack_into('<8h', header, 40, 3, *size, 1, 1, 1, 1)  # dimensions
struct.pack_into('<h', header, 70, 16)  # float32
struct.pack_into('<h', header, 72, 32)  # bitpix
struct.pack_into('<8f', header, 76, 1.0, 0.08, 0.08, 0.08, 1, 1, 1, 1)  # pixdim (mm)
struct.pack_into('<f', header, 108, 352.0)  # vox_offset
struct.pack_into('<h', header, 252, 0)  # qform_code
struct.pack_into('<h', header, 254, 1)  # sform_code
struct.pack_into('<4f', header, 280, 0.08, 0, 0, -3.84)
struct.pack_into('<4f', header, 296, 0, 0.08, 0, -3.2)
struct.pack_into('<4f', header, 312, 0, 0, 0.08, -2.56)
header[344:348] = b'n+1\x00'

out = Path('sample.nii.gz')
with gzip.open(out, 'wb', compresslevel=1) as fp:
    fp.write(header)
    for z in range(size[2]):
        for y in range(size[1]):
            row = []
            for x in range(size[0]):
                dx = (x - 48) / 37
                dy = (y - 40) / 30
                dz = (z - 32) / 23
                d = dx*dx + dy*dy + dz*dz
                blob = 800 * math.exp(-5*((x-33)/12)**2 - 3*((y-43)/15)**2 - 5*((z-32)/11)**2)
                pattern = 120 * (1+math.sin(x/7)*math.cos(y/9)*math.sin(z/7))
                row.append((pattern + blob) if d < 1 else 0.0)
            fp.write(struct.pack('<' + 'f'*size[0], *row))
print('Wrote', out.resolve())
