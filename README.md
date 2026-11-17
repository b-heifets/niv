# niv

Fast, keyboard-driven grayscale NIfTI slice viewer for Ghostty, Rex, and other
terminals supporting the [Kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/).
Standalone Rust binary. Runs on macOS and Linux (including remote Rex/Tailscale
sessions). No libghostty, Python runtime, or direct Rex integration is required.

> **v0.3 experimental release.** The Rust compiler and a compatible graphical
> terminal were not available in the development environment, so the Rust build
> and interactive appearance still need testing locally. NIfTI physical/anatomical
> reorientation and atlas contours are not implemented yet.

## Build and install

Install [Rust](https://rustup.rs/) if needed, then run from the project root:

```sh
cargo test
cargo install --path . --force
```

Run in Rex/Ghostty (this works from your atlas directory):

```sh
niv atlas_CCFv3_2020_30um.nii.gz --axis x --rotate 90
```

Or directly from the source tree after `cargo build --release`:

```sh
./target/release/niv data/sample.nii.gz --rotate 90
```

To set defaults in zsh, add to `~/.zshrc`:

```sh
export NIV_AXIS=x
export NIV_ROTATE=90
```

CLI `--axis` and `--rotate` take precedence over environment variables.
`NII_AXIS` is accepted as a legacy fallback for `NIV_AXIS`.

## Keyboard controls

| Key | Action |
| --- | --- |
| `↑` / `→` / `k` / `l` | Next slice (+1 by default) |
| `↓` / `←` / `j` / `h` | Previous slice (−1 by default) |
| Alt + navigation key | ×2 |
| Shift + navigation key or uppercase `HJKL` | ×5 |
| Ctrl + navigation key | ×10 |
| `r` / `R` | Rotate screen image clockwise / counterclockwise by 90° |
| `x`, `y`, `z` | Change voxel-index slicing axis |
| `a` | Cycle index axes |
| `f` / `v` | Flip horizontally / vertically *on screen* |
| `+` / `-` | Narrow / widen grayscale window |
| `,` / `.` | Shift window down / up |
| `0` | Reset automatically chosen or initial intensity window |
| `[` / `]` | Halve / double **stride** (base slice increment) |
| PageUp / PageDown | Move ±25 slices |
| Home / End | First / last slice |
| `q`, Esc, Ctrl+C | Quit |

**Stride** defaults to 1 and is only shown in the header when greater than 1.
If stride = 4, an unmodified navigation key moves 4 slices, Alt moves 8,
Shift moves 20, and Ctrl moves 40. Use `[` to decrease stride again.
The CLI spelling `--step` is retained for compatibility.

Key modifier support depends on the terminal and Rex's keyboard forwarding.
Some combinations such as Ctrl+h are traditionally Backspace. If modifier
reporting causes trouble, try `--no-enhanced-keys`.

## Display

The top bar shows filename, `X×Y×Z` voxel dimensions, index axis,
zero-based slice index, clockwise screen rotation, and stride only if not 1.
There is no `niv` prefix.

Below the image:

```text
data: 0.000..1327.000 | display: 9.000..1300.000
arrows/hjkl slices | Alt 2x Shift 5x Ctrl 10x | xyz axis | r/R rotate | f/v flip | +/- contrast
```

- **data**: exact finite minimum and maximum of the loaded 3D volume,
  including zero-valued background; calculated once at load time.
- **display**: actual minimum/maximum grayscale window. Values at or below the
  lower bound are black; those at or above the upper bound are white.
  These values change with `+/-` and `,/.` and reset with `0`.

The first window is chosen from a sparse sample of nonzero voxels (approximately
1st and 99.5th percentiles). The file is never modified.

`--rotate` accepts only `0`, `90`, `180`, or `270`, measured clockwise in the
terminal image. Rotation and flips are **display-only**; they do not interpret
or change NIfTI sform/qform anatomical orientation.

## Options

```text
-a, --axis x|y|z          Slice along raw NIfTI array index (default: NIV_AXIS / NII_AXIS / z)
-r, --rotate 0|90|180|270  Clockwise display rotation (default: NIV_ROTATE / 0)
    --slice N             Start at zero-based slice N (default: center)
    --step N              Base slice stride (1..256; default 1)
-w, --window LOW,HIGH     Manual grayscale window
    --max-pixels N        Maximum transmitted raster pixels (default 350000)
    --fps N               Maximum redraw FPS (default 30)
    --no-enhanced-keys    Disable enhanced keyboard mode
    --bench N             Time N slice render/encode operations; don't write graphics
-h, --help                Show help
-V, --version             Version
```

Examples:

```sh
niv brain.nii.gz --axis z --rotate 90
niv brain.nii.gz --axis y --window 0,3000
niv brain.nii.gz --max-pixels 120000 --fps 12   # Remote Rex/Tailscale
niv brain.nii.gz --bench 100                     # No terminal image needed
```

## Flicker and performance

v0.2 removed the previous Kitty image and cleared the *entire* terminal on
**every** frame. That created a blank background between successive slices.

v0.3 avoids clearing the image region during normal redraws: it gives each
new frame a fresh Kitty image ID, sends and places the complete image **while
keeping the previous image visible**, then deletes the previous image. Only
the three text lines are cleared/updated. This reduces the source of flashing
observed in v0.2, but real smoothness still depends on Ghostty/Rex rendering,
network bandwidth, and protocol buffering. A terminal might not present all
updates atomically; please test both locally and through Rex/Tailscale.

Internally `.nii.gz` is decompressed once to an `ndarray::Array3<f32>` (~170 MiB
for a 440×267×380 volume; peak memory during load is higher). All axes can
then be sampled directly. The image is rendered with nearest-neighbor sampling,
fast zlib RGB24 compression, and Kitty graphics in <=4096-character chunks.
NIfTI `scl_slope`/`scl_inter` are handled by the reader. A `uint16` input does
not overflow because the internal grayscale computation uses `f32`.

The terminal image resolution is capped independently from the text-cell
placement size. Remote performance can be improved using `--max-pixels` and
`--fps` without changing the source image or the selected slice.

## Troubleshooting

- If you still have v0.2 in `$PATH`, reinstall with `cargo install --path . --force`.
  Check `niv --version` (expect `niv 0.3.0`) and `which niv`.
- If the image fails entirely in a Rex session, run `python3 examples/kitty_probe.py`
  in the **same session** to test graphics protocol support.
- If remote updates flash or lag, try `--max-pixels 120000 --fps 12`.
- If `r`/`R` are flipped relative to your preference, use `--rotate 270` initially.
- For compile issues, run `cargo test` and `cargo build --release` and share
  the error output; this release was not compiled in its creation environment.

## Source layout

- `src/config.rs`: CLI and environment defaults
- `src/volume.rs`: NIfTI loading, scale factors, voxel sampling, ranges
- `src/render.rs`: view geometry, 90° rotations, flips, nearest-neighbor RGB
- `src/kitty.rs`: zlib/base64 frame encoding, Kitty image transmission and replacement
- `src/app.rs`: navigation state, stride, contrast, key events
- `src/main.rs`: event loop, double-frame lifetime, terminal UI, benchmark
- `examples/kitty_probe.py`: self-contained graphics probe
- `examples/make_sample.py`: sample generator (standard-library Python)
- `data/sample.nii.gz`: synthetic test volume

Current limitations: raw index axes rather than anatomical reorientation;
only one 3D image at a time; no atlas overlay, region inspector, or world-space
coordinates. The volume and associated image data are read-only.

MIT license.
