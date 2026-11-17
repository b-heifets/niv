# niv

**A fast, keyboard-driven, grayscale NIfTI slice viewer for Ghostty and Rex.**

Standalone Rust binary; no Python runtime, GUI framework, libghostty, or direct Rex API needed. The only terminal graphics dependency is **Kitty graphics protocol**. Intended for macOS (Ghostty/Rex) and Linux machines accessed through Rex/Tailscale.

> Status: **v0.2 prototype.** The source has unit tests, but the creation environment had no Rust toolchain or terminal to compile it or visually test it. Compile/test locally before relying on it. Anatomical orientation is **not yet implemented**; see limitations.

## Quick start

Install Rust if needed: <https://rustup.rs/>. Then, from the extracted project directory:

```sh
cargo install --path .
```

`~/.cargo/bin` should be on your zsh `$PATH`. If it is not:

```sh
export PATH="$HOME/.cargo/bin:$PATH"
```

Run the included sample:

```sh
niv data/sample.nii.gz
```

Or your volume:

```sh
niv /path/to/brain.nii.gz
niv /path/to/brain.nii.gz --axis y
niv /path/to/brain.nii.gz --axis z --window 0,3000
```

Optional environment default (put in `~/.zshrc` if wanted):

```sh
export NIV_AXIS=z
```

Legacy `NII_AXIS` is also recognized; `NIV_AXIS` takes precedence, and CLI `--axis` overrides both.

### Keyboard reference

| Key(s) | Behavior |
| --- | --- |
| `↑`, `→`, `k`, `l` | Next slice (increasing voxel index) |
| `↓`, `←`, `j`, `h` | Previous slice |
| `Shift`+arrow or `H J K L` | Movement ×10 |
| `Ctrl`+arrow | Movement ×25 |
| `Alt`+arrow | Movement ×50 |
| `[` / `]` | Halve / double persistent movement step (1–256) |
| `PageUp` / `PageDown` | Move +25 / −25 slices |
| `Home` / `End` | First / last slice |
| `x` / `y` / `z` | Switch voxel index axis; preserve fractional slice position |
| `a` | Cycle voxel axes |
| `f` / `v` | Flip horizontally / vertically |
| `+` / `-` | Narrow / widen grayscale window (contrast) |
| `,` / `.` | Shift intensity window down / up (brightness) |
| `0` | Reset intensity window |
| `q` / `Esc` / `Ctrl+C` | Quit |

Terminal/multiplexer key forwarding varies; `Ctrl+h` and `Ctrl+j` can conflict with Backspace/Return. Shift+arrow, uppercase vim keys, and `[ ]` are reliable alternatives. If enhanced key reporting behaves unexpectedly, use `--no-enhanced-keys`.

### Options

```text
-a, --axis x|y|z         slice along array X/Y/Z (default NIV_AXIS, NII_AXIS, z)
    --slice N            start at zero-based index N (default middle)
    --step N             initial movement step (1..256)
-w, --window LOW,HIGH    fixed display intensity limits
    --max-pixels N       rendered image pixel budget, default 350000
    --fps N              cap drawing frames per second, default 30
    --no-enhanced-keys   disable Kitty enhanced-keyboard negotiation
    --bench N            encode N frames without terminal output; print timings
-h, --help               full CLI help
```

Automatic display range uses a sparse deterministic sample of finite nonzero voxels (approximately 1st–99.5th percentile). The viewer does **not modify** source data. NIfTI `scl_slope` / `scl_inter` are respected by the `nifti` loader, and data are stored as `f32` in RAM so uint16 input won't wrap or overflow during rendering.

## Run through Rex / Tailscale

1. Install the binary **on the machine where you execute `niv`**. For a remote Linux Rex session, build `niv` on that Linux machine (`cargo install --path .`) or copy a Linux-compatible binary.
2. Start it in a Rex terminal block. No `TERM=kitty` spoofing is needed: the program writes the Kitty graphics protocol directly.
3. Try `python3 examples/kitty_probe.py` in the *same session* if images fail to appear. It tests whether the Kitty graphics response comes back through Rex.
4. For a slower remote connection, reduce per-frame transfer size and frame rate:

   ```sh
   niv volume.nii.gz --max-pixels 120000 --fps 12
   ```

Rendering can be fast even if the terminal or network takes time to transmit/decode frames. Direct zlib-compressed RGB frames are base64-encoded into 4096-byte chunks to pass safely over terminal sessions.

**Performance test without graphics:**

```sh
niv data/sample.nii.gz --bench 100
niv /path/to/brain.nii.gz -a z --bench 50 --max-pixels 120000
```

The benchmark measures rasterization, zlib compression, and base64 encoding; it **does not** measure the Rex/network/terminal graphics path.

## Memory and speed

- `.nii.gz` is fully decompressed/decoded **once** into a 3D `ndarray::Array3<f32>`; thereafter switching X/Y/Z planes is O(number of displayed pixels), not O(size of volume).
- A `440 × 267 × 380` volume occupies **~170 MiB** for the `f32` voxel array. Peak load memory is higher because the compressed/decompressed source and conversion buffers may temporarily coexist.
- A uniform volume is windowed to avoid division by zero; NaN and infinite pixels render black.
- The raster uses **nearest-neighbor** sampling, and its aspect ratio uses NIfTI voxel spacing. If the image has fewer pixels than the viewport, some nearest-neighbor upsampling happens before transfer.
- The renderer limits both the image's pixel count and redraw rate, and coalesces bursts of keyboard events. `--max-pixels` and `--fps` can be tuned separately.
- Compression uses `flate2` with a fast zlib setting; Kitty graphics uses RGB24 plus direct transfers (`f=24`, `o=z`, `t=d`).

## Limitations / development roadmap

1. **Voxel axes only.** `x/y/z` correspond to array index axes; the display is *not* yet reoriented to anatomical RAS/LPS/PIR, nor does it apply `sform`/`qform` to reslice oblique volumes. `f`/`v` manually change visual flips but don't create correct anatomical labels. If axes are aligned to an atlas, the index slices are still useful.
2. **3D single-volume only.** 4D/time series with more than one frame are not supported. Singleton trailing dimensions are accepted. No atlas overlay yet.
3. **No mouse or cursor value inspector** yet. The status line displays zero-based slice index and axis, not physical coordinates.
4. Kitty graphics support is **assumed**. Terminal/remote-multiplexer versions that do not pass the protocol will show text but no image; there is no ANSI fallback in this version. Use `examples/kitty_probe.py` to check.
5. Full-volume memory loading can be expensive for giant images. For those, a later mode could decompress once to an on-disk uncompressed `.nii` and memory map it; lazy access directly into arbitrary planes of a standard `.nii.gz` is not efficient.
6. Future atlas overlay: load a separate Allen CCFv3 2020 label volume, align it in world coordinates using affines, draw nearest-neighbor boundaries as an RGB overlay, and display region acronym/name/color/ID in an information panel.

## Development

```sh
# Recreate a synthetic 96x80x64 float32 NIfTI using only the Python stdlib
python3 examples/make_sample.py
# The generated file appears in the current working directory as sample.nii.gz

cargo fmt
cargo test
cargo clippy --all-targets -- -D warnings
cargo build --release
./target/release/niv data/sample.nii.gz
```

### Code organization

- `src/config.rs`: CLI and environment configuration
- `src/volume.rs`: NIfTI decoding/scaling, axis indexing, automatic intensity window
- `src/render.rs`: viewport geometry, grayscale nearest-neighbor RGB rasterization
- `src/kitty.rs`: fast zlib+base64 Kitty image transmission and selective cleanup
- `src/app.rs`: view state and keybindings
- `src/main.rs`: terminal lifecycle, status display, redraw scheduling and benchmark
- `examples/kitty_probe.py`: self-contained protocol support test
- `examples/make_sample.py`: synthetic test NIfTI generator

### References

- [NIfTI-rs `nifti` 0.18](https://docs.rs/nifti/0.18.0/nifti/)
- [Crossterm 0.29](https://docs.rs/crossterm/0.29.0/crossterm/)
- [Kitty terminal graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/)
- [Ghostty terminal features](https://ghostty.org/docs/features)

MIT-licensed. No data files are modified by the viewer.
