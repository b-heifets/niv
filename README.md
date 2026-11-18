# niv v0.14

Fast, keyboard-driven NIfTI stack viewer for Ghostty / Rex / Kitty-graphics terminals, written in Rust. Loads a `.nii` or `.nii.gz` volume into RAM, draws grayscale or color-mapped slices, composites additional aligned intensity layers, and optionally draws colored CCF atlas contours above them.

## Install / upgrade

```bash
cd ~/Documents/_GitHub/niv
# Extract the v0.14 source-only update here, or apply its patch if it fits your checkout.
cargo fmt
cargo test
cargo install --path . --force
niv --version       # v0.14.0
```

Both ZIP files contain paths relative to their extraction directory. The full project ZIP includes sample data and examples; the source-only ZIP includes updated Rust source files, `Cargo.toml`, `README.md`, and an example scene, ready to extract into your existing repository. Don't apply the patch *and* extract the ZIP. Save a Git commit before upgrading if you've made local edits.

## Viewer

```bash
niv brain.nii.gz                           # defaults to --axis x --rotate 90
niv brain.nii.gz --overlay activation.nii.gz
niv brain.nii.gz -i activation_a.nii.gz activation_b.nii.gz
niv brain.nii.gz -i '*zstat*.nii.gz' '*clusters*.nii.gz'
niv brain.nii.gz -i 'stats/*fstat*.nii.gz' -i 'other/*tstat*.nii.gz'
niv brain.nii.gz --background transparent # start in RGBA mode
niv brain.nii.gz --hover-delay-ms 250       # delay before thicker hovered-region contour
niv brain.nii.gz --atlas-aa                 # optional smoother atlas edges
niv brain.nii.gz --fps 20 --profile        # if Rex still queues images after key release
```

The screen has a colored region name at the upper left, `Press ? for help` at upper right, voxel indices and intensity values for every layer on the next row, the image, and a **single selected-layer status line below the image**. It shows `1: image  |  min: …  |  max: …  |  α: …`. Names use a middle ellipsis if space is tight; `t` reveals a list of layers in **top-to-bottom order**, while `I` (Shift+I) shows full metadata. No persistent shortcut footer is shown. Loading messages/errors or typed range/opacity editors temporarily appear at the bottom. The `n` file picker temporarily replaces the image with a filterable list, without continually redrawing the NIfTI raster.

### Keys

| Key | Function |
| --- | --- |
| `↑↓←→`, `hjkl`, mouse wheel | Scroll volume slices |
| Command + direction | ×2 where Rex/terminal forwards Command |
| Shift + direction | ×3 |
| Control + direction | ×4 |
| Option/Alt + direction | ×2 fallback for Command |
| `[` / `]` | Decrease/increase base stride by one |
| `b` | Toggle **black/RGB** ↔ **transparent/RGBA** background |
| `a` | Toggle atlas outlines |
| `g` | Toggle optional antialiasing for **solid-label atlas** contours (off by default) |
| `?` or `/` | Popup keyboard help |
| `I` (Shift+I) | Metadata and full layer names; works with Kitty enhanced Shift+i |
| `n` | Open a searchable file picker; type any filename substring (case-insensitive); ↑↓ select, Tab complete/cycle, → enter directory, Space mark multiple, Enter load |
| `1`–`9`; `;` | Select image by current stack position (1 = bottom); cycle beyond nine layers |
| `{` / `}` (Shift+`[` / Shift+`]`) | Move selected image down / up one position in the stack |
| `e`, `m`, `M` | Edit selected-layer range / only min / only max |
| `0` | Reset range to actual finite min/max |
| `o`; `O` | Edit selected-layer opacity; edit atlas opacity |
| `c` | Cycle gray → warm → cool → categorical integer-ID palette |
| `t` | Show/hide layer list (top image first; Esc closes the list) |
| Space | Toggle selected overlay visibility (original base image stays visible) |
| `i` | Keyboard inspection crosshair (arrows move probe) |
| `x`,`y`,`z` | Change array slicing axis |
| `r`,`R` | Rotate clockwise / counterclockwise |
| `f`,`v` | Flip horizontally / vertically |
| Tab | Hide/show viewer text, fill terminal |
| `q` | Quit (Esc dismisses popup/editor) |

**Important:** Command/Super may be reserved by Ghostty, Rex, or the OS and not arrive as a terminal key modifier. Option/Alt has the same ×2 mapping as a fallback. Shift/Ctrl/Option support likewise depends on which input sequences your Rex client forwards.

### Loading many images

The first positional file is the reference volume. Use **`-i` / `--images`** to append any number of aligned intensity layers, including multiple distinct glob patterns. These are expanded by `niv` even when quoted (recommended in zsh to avoid shell `nomatch`); each pattern's matches are sorted and overlapping matches are deduplicated.

```bash
niv mean_IF.nii.gz -i 'stats/*_tstat*.nii.gz' 'stats/*_fstat*.nii.gz'
niv mean_IF.nii.gz -i image_1.nii.gz image_2.nii.gz --atlas atlas.nii.gz
niv mean_IF.nii.gz -i 'a/*.nii.gz' -i 'b/*.nii.gz'
```

New layers use grayscale and **100% opacity** by default. The legacy `--overlay FILE` flag still creates a warm-tinted layer and supports `--overlay-color`, `--overlay-window`, and `--overlay-opacity` options. The atlas stays on top and all images must share the reference volume's voxel grid.

When already viewing an image, press **`n`** to open the file picker. Type any substring of the filename, including middle words such as `fstat` or `cluster`. The picker lists all current-directory matches, with directories first. Use ↑/↓ (or PgUp/PgDn) to highlight, Tab/Shift+Tab to complete or cycle, → to enter a directory, and **Space** to mark multiple images. Press Enter to load marked files or the highlighted file. Press Esc to return to the viewer. Full paths appear on the picker's selection line. This completion runs inside `niv`, not the shell; it doesn't require zsh or Oh My Zsh. It searches one directory at a time for responsiveness, rather than recursively indexing the entire filesystem.

### How backgrounds, thresholds and opacity work

**Black / RGB (default)**: Each NIfTI layer is composited into an opaque black RGB canvas **in memory**. Values at/below a layer's display minimum are skipped, so that they reveal lower layers. Partial image/atlas opacity blends with lower layers (or black). Final empty voxels are black. The terminal receives Kitty `f=24` compressed RGB bytes (3 channels/pixel).

**Transparent / RGBA (`b`)**: Layers are composited in memory using straight alpha, with empty voxels remaining transparent. The terminal receives Kitty `f=32` (4 channels/pixel), allowing the Ghostty background to appear through empty pixels. Per-layer opacity and threshold behavior are the same in both modes. True final-terminal transparency requires RGBA, but **multiple-layer transparency does not**.

`NIV_BACKGROUND=black|transparent` or `--background black|transparent` sets the startup mode. Black is the faster default. Performance can differ due to compression ratios and terminal/Rex transmission; it is **not guaranteed** to be exactly 25% faster.

### Hovered-region highlight

The feature is enabled by default with a 20ms delay. When enabled, with a **solid** atlas, entering a labeled region starts a short delay. If the pointer remains in that region past the delay, its contours are drawn thicker (3px) while other regions retain their usual outline thickness. When enabled, hover events *before* the delay only update the text line; they do not re-transmit the image. Moving between voxels of the same region does not restart the timer.

Use `NIV_HOVER_DELAY_MS=20` (default) or `--hover-delay-ms 150` to tune, 0–2000 ms; `0` disables the highlight. The thicker outline needs one full redraw when the region changes. Its antialiasing now follows `g`, and the atlas strokes are composited once per pixel to prevent bright intersections. For precomputed wireframe outlines, thickening requires companion solid labels (`--atlas-labels`); without them, the outline is unchanged. A keyboard probe remains available with `i` if iOS Rex does not forward mouse-move events.

### Environment variables

```zsh
export ATLAS="$HOME/Documents/__Heifets_lab_data/_UNRAVEL/atlases/CCFv3-2020/30um/atlas_CCFv3_2020_30um.nii.gz"
export ATLAS_LUT="$HOME/Documents/_GitHub/UNRAVEL/unravel/core/csvs/CCFv3-2020_regional_summary.csv"
export NIV_BACKGROUND=black
export NIV_HOVER_DELAY_MS=20
export NIV_PIXEL_BUDGET=1000000
export NIV_FPS=30
export NIV_ATLAS_AA=0
```

CLI `--atlas` / `--atlas-wireframe` and `--lut` override the environment. `--axis x --rotate 90` are defaults. Image and atlas NIfTI files must share identical dimensions, voxel spacing, and sform (when present); no resampling/reorientation is performed.

### Performance and profiling

The default pixel budget is **1,000,000 output pixels** (the display placement retains its full terminal footprint). Lower it for high-latency Rex/Tailscale connections, or set `--pixel-budget 0` for native terminal resolution. `NIV_PIXEL_BUDGET` sets the default. The CPU renderer caches the screen-to-voxel sampling map between slice changes. Atlas strokes use a one-pass coverage buffer: line corners and intersections are no longer brightened by repeated alpha blending. For antialiasing, the centerline remains between the same voxel faces and only a soft fringe changes coverage; it does not smooth the actual stair-step label geometry. Repeated navigation keys already buffered before a redraw are discarded after one navigation increment; different keys and Quit are preserved, while a matching key-release simply ends the repeat burst. A pending image is presented before another navigation event is handled. This reduces delayed scrolling after key release, although Rex/Ghostty may still queue images asynchronously. The default redraw cap is **30 FPS** (`NIV_FPS` or `--fps`), rather than 60 FPS, to reduce terminal image backlog. Hover-region highlighting is enabled with a short 20ms delay; set `NIV_HOVER_DELAY_MS=0` to disable it and avoid those extra redraws.

Use `--profile` with interactive viewing. After quitting, it reports average **rasterization**, **zlib/base64 encoding**, and **terminal write/flush** durations plus compressed payload bytes. This does not measure asynchronous terminal decode or network rendering latency.

```bash
niv brain.nii.gz --profile --pixel-budget 1000000
niv brain.nii.gz --profile --pixel-budget 0
niv brain.nii.gz --bench 100
```

Kitty can transmit 8-bit terminal palette indices, but those colors are tied to the terminal palette and are unsuitable for the atlas LUT and arbitrary layer colors. Paletted PNG is another future transport to benchmark, but entails encoding/decoding costs and would not be universally faster.

### Benchmark RGB vs RGBA

```bash
niv data/sample.nii.gz --background black --bench 100
niv data/sample.nii.gz --background transparent --bench 100
```

`--bench` reports **rasterization plus zlib encoding without terminal transmission**. Compare with actual scrolling through local and remote Rex to find whether CPU rendering, encoded bytes, or terminal transfer is the bottleneck. The application is currently CPU-rendered; it does **not** use multi-threading or a GPU. For the current CPU/Kitty pipeline, avoiding alpha bytes and unnecessary redraws is a low-complexity optimization worth testing before a GPU rewrite.

## Limits

- `float64` NIfTI data are converted to `f32` (hover shows seven *decimal places*, not guaranteed seven significant digits of source precision).
- True transparent pixels require Kitty RGBA support in the terminal/Rex client. Image opacity works in RGB mode too, but with black as the final background.
- The optional `--atlas-aa` (or `g` / `NIV_ATLAS_AA=1`) uses improved centered fractional-coverage subpixel line edges for contours generated from the **solid** atlas, which can soften jagged appearance at low pixel budgets. It is off by default because it adds raster work. It is **not** vector tracing or spline smoothing, and it does not change atlas labels or underlying voxel geometry. For precomputed wireframes, `g` can soften the dilated **hovered-region** outline when companion solid labels are loaded, but does not smooth the original mask. Hover selection draws a thicker 3-pixel contour when enabled.
- Runtime `n` image loading and `.nii.gz` decompression are synchronous; loading several large images can briefly block the UI.
- Hover events, Command modifier events and pixel-mouse tracking are client-dependent.
- `cargo test` and interactive visual testing should be run locally; this source distribution was assembled where Rust's toolchain was unavailable.

### Reordering image layers

Select an image with its current stack number (`1` is bottom), or cycle with `;`. Press **`}`** (Shift+`]`) to raise the selected image one position, or **`{`** (Shift+`[`) to lower it. The selection follows the moved image; its color map, opacity, display window, and visibility state remain attached to it. Press `t` for the top-to-bottom layer list (atlas remains above all images). Press `Esc` once to close the list; a second `Esc` exits `niv`. The original base image can move above other intensity images, but the **atlas always stays on top**. Reordering does not copy or reload any image volume; it only changes compositing order and redraws the image. Newly loaded images are appended immediately below the atlas.

## Save and reload scenes (v0.14)

Press **`s`** in the viewer to save your current scene. An editable filename prompt
appears, defaulting to `<base-name>.niv.toml` in the current working directory,
or the current scene filename if you loaded a scene. Press **Enter** to save or
**Esc** to cancel. Saving to an existing filename replaces that scene file atomically. The viewer
requires the `.niv.toml` suffix to prevent accidentally overwriting NIfTI data.

```bash
niv brain.nii.gz -i zstat1.nii.gz zstat2.nii.gz  # browse and configure layers
# press s, edit the scene filename if desired, then Enter
niv --scene brain.niv.toml                         # restore the scene
niv --scene brain.niv.toml --pixel-budget 200000   # optional performance override
niv --scene brain.niv.toml --fps 20 --profile
niv --scene examples/demo_scene.niv.toml           # bundled demonstration
```

Scenes are **plain TOML text**, not shell scripts. You can edit them in Neovim,
share them with collaborators, or commit them to Git (they don't embed image
bytes). They restore the base and additional NIfTI paths, **top-to-bottom layer
ordering**, each layer's visibility, grayscale/color palette, display window
and opacity, the atlas/LUT and atlas settings, slice/axis/rotation/flips,
selected layer, inspection state, and performance defaults.

`[[layers]]` entries are stored **top-to-bottom**, matching `t`; exactly one has `role = "base"`.
The atlas is represented separately and always remains the topmost layer.
`view.selected` is zero-based (0 is the top image).
File paths below the scene directory are saved as **relative paths**, so moving
the scene and its data directory together works. Files elsewhere are saved as
absolute paths. On reload, relative paths are resolved from the scene location,
regardless of the shell working directory. A scene is intended for files on the
**same registered voxel grid**; it does not embed or resample NIfTI data.

Use `niv --scene FILE.niv.toml` as a self-contained input mode. It deliberately
rejects mixing in `-i`/`--atlas`/image paths; edit/save the scene or load images
interactively instead. `--fps`, `--pixel-budget`, `--profile`, and `--bench`
may be supplied alongside `--scene`. Standard image-loading commands remain
unchanged.

Scene files may include local research paths, so review them before sharing.
