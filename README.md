# niv v0.19

Fast, keyboard-driven NIfTI stack viewer for Ghostty / Rex / Kitty-graphics terminals, written in Rust. Loads a `.nii` or `.nii.gz` volume into RAM, draws grayscale or color-mapped slices, composites additional aligned intensity layers, and optionally draws colored CCF atlas contours above them.

## Install / upgrade

```bash
cd ~/Documents/_GitHub/niv
# Extract the latest source-only update here.
cargo fmt
cargo test
cargo install --path . --force
niv --version       # 0.22.0
```

Both ZIP files contain paths relative to their extraction directory. The full project ZIP includes sample data and examples; the source-only ZIP includes updated Rust source files, `Cargo.toml`, `README.md`, and an example scene, ready to extract into your existing repository. Don't apply the patch *and* extract the ZIP. Save a Git commit before upgrading if you've made local edits.

## Viewer

```bash
niv brain.nii.gz                           # defaults to --axis x --rotate 90
niv brain.nii.gz activation.nii.gz
niv -i brain.nii.gz activation.nii.gz
niv brain.nii.gz --overlay activation.nii.gz
niv brain.nii.gz -i activation_a.nii.gz activation_b.nii.gz
niv brain.nii.gz -i '*zstat*.nii.gz' '*clusters*.nii.gz'
niv brain.nii.gz -i 'stats/*fstat*.nii.gz' -i 'other/*tstat*.nii.gz'
niv brain.nii.gz --background transparent # start in RGBA mode
niv brain.nii.gz --hover-delay-ms 250       # delay before thicker hovered-region contour
niv brain.nii.gz --atlas-aa                 # optional smoother atlas edges
niv brain.nii.gz --fps 20 --profile        # if Rex still queues images after key release
```

The screen has a colored region name at the upper left, `Press ? for help` at upper right, voxel indices and intensity values for every layer on the next row, the image, and a **single selected-layer status line below the image**. It shows `1: image  |  min: …  |  max: …  |  α: …`. Names use a middle ellipsis if space is tight; `t` reveals a list of layers in **top-to-bottom order**, while `i` shows full metadata. No persistent shortcut footer is shown. Loading messages/errors or typed range/opacity editors temporarily appear at the bottom. The `n` file picker temporarily replaces the image with a filterable list, without continually redrawing the NIfTI raster.

### Keys

| Key | Function |
| --- | --- |
| `↑↓←→`, `hjk`, mouse wheel | Scroll volume slices |
| Command + direction | ×2 where Rex/terminal forwards Command |
| Shift + direction | ×3 |
| Control + direction | ×4 |
| Option/Alt + direction | ×2 fallback for Command |
| `[` / `]` | Decrease/increase base stride by one |
| `X` / `A` | Enter projection; set selected image max / average (`V` returns to slices) |
| `p` | Toggle paged montage view (atlas and layers are retained) |
| `B` / `E` | Edit montage start / stop |
| `[` / `]` in montage | Decrease / increase slice spacing by one index |
| `+` / `-` in montage | Zoom in / out (fewer / more rows and columns) |
| `<` / `>` in montage | Decrease / increase tile gap by one output pixel |
| `b` | Toggle **black/RGB** ↔ **transparent/RGBA** background |
| `a` | Toggle atlas outlines |
| `g` | Toggle optional antialiasing for **solid-label atlas** contours (off by default) |
| `?` or `/` | Popup keyboard help |
| `i` | Metadata and full layer names |
| `n` | Open a searchable file picker; type any filename substring (case-insensitive); ↑↓ select, Tab complete/cycle, → enter directory, Space mark multiple, Enter load |
| `1`–`9`; `;` | Select image by current stack position (1 = top); cycle beyond nine layers |
| `w` / `{` (or `{` / `}`) | Raise / lower selected image |
| `d` | Unload selected image (keep at least one reference image) |
| `Y` | Copy selected image path to clipboard and yank display settings |
| `X` in `t` | Mark selected image for a move |
| `p` / `P` in `t` | Paste below / above selected image (move if cut; duplicate if yanked) |
| `e`, `m`, `M` | Edit selected-layer range / only min / only max |
| `0` | Reset range to actual finite min/max |
| `o`; `O` | Edit selected-layer opacity; edit atlas opacity |
| `l` | Cycle gray → warm → cool → categorical integer-ID palette |
| `t` | Show/hide layer list (top image first); ↑/↓ or `k`/`j` selects, Enter / q / Esc closes and retains the selection |
| Space | Toggle selected image visibility (including the original base) |
| `I` (Shift+i) | Toggle keyboard inspection crosshair / pointer hover |
| `x`,`c`,`z` (`y` alias) | Change array slicing axis; repeat any axis key to mirror horizontally |
| `r`,`R` | Rotate clockwise / counterclockwise |
| `f`,`v` | Flip horizontally / vertically |
| Tab | Hide/show viewer text, fill terminal |
| `q` | Close menu or numeric editor; quit in normal viewer |
| Ctrl+C | Quit from any interactive menu or editor |

**Important:** Command/Super may be reserved by Ghostty, Rex, or the OS and not arrive as a terminal key modifier. Option/Alt has the same ×2 mapping as a fallback. Shift/Ctrl/Option support likewise depends on which input sequences your Rex client forwards.

### Montage mode (v0.15)

Press **`p`** to open a montage for figure preparation. The default is a
**3-row × 4-column** grid with 8-raster-pixel gaps and up to 12 evenly spaced
slices including the bounds. If a solid atlas (or companion solid labels) is
loaded, bounds trim about 10% from each end of the occupied atlas along the
current array axis. Otherwise the central 80% of the reference grid is used.
There are no gold selection borders or slice numbers by default.

| Key | Effect |
| --- | --- |
| `p` / `P` | Toggle montage / reset to preset |
| `B` / `E` | Edit inclusive start / stop (zero-based slice indices) |
| `Z` / `F` | Edit columns / rows (1–12) |
| `U` | Edit slice interval (0 = automatic 12 slices, 1–256 manual) |
| `D` | Edit tile gap (0–32 output pixels) |
| `+` / `-` | Zoom in/out by decreasing/increasing grid dimensions |
| `[` / `]` | Adjust manual slice interval |
| `<` / `>` | Adjust tile gap |
| ↑/↓ or wheel | Move the visible window by one row when more slices exist |
| ←/→ | Move one montage position; PgUp/PgDn move one grid |
| `N` | Toggle slice numbers |
| `C` | Toggle numeric cluster IDs |
| `Tab` | Image-only mode for screenshots |
| `?` or `/` | Dedicated montage help |

For numeric editors, type a replacement value or use arrows to change the
current draft. Enter applies; Esc cancels; Ctrl+U clears the draft. The grid
fits both screen dimensions without changing aspect ratio. Manual intervals
show additional rows when needed. The last slice is included when on the
specified manual interval; automatic mode includes both endpoints.

Cluster labels use the selected visible categorical image (`c` cycles to the
categorical palette), or the topmost visible categorical image if the selection
is not categorical. Positive or negative integer IDs other than zero above
the display threshold are labeled once per slice, using the same color as the
cluster. The renderer anchors labels near the cluster centroid at an actual
cluster pixel, prioritizes larger regions, and omits colliding or oversized
labels. Atlas acronym labels are not drawn by this toggle.

**Atlas Gaussian blur:** `G` opens a numeric sigma editor (0–4 output pixels,
0 disables it; arrows change by 0.25 px). `--atlas-blur SIGMA` or
`NIV_ATLAS_BLUR` sets the initial value for ordinary image loading. The filter
blurs premultiplied color and alpha of the wireframe only, including hover
strokes, preserving the underlying intensity rasters. It can be used with or
without the original `g` edge antialiasing. Blur and cluster annotations cost
additional time; both default off for fast browsing. Blur can soften/widen
contours and mix neighboring region colors without changing label IDs.

**Performance and scenes:** The montage is one Kitty image, and its total
raster is capped by the pixel budget. Only the visible rows are rendered.
`s` saves the range, grid, gap, label toggles, and atlas blur with normal scene
settings; older scenes remain readable (new fields default to off or 3 rows).

### Loading many images

The first ordinary image (positional or supplied with **`-i` / `--images`**) is the reference volume. Further images from either form become aligned intensity layers; multiple distinct glob patterns are supported. These are expanded by `niv` even when quoted (recommended in zsh to avoid shell `nomatch`); each pattern's matches are sorted and overlapping matches are deduplicated.

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

Use `NIV_HOVER_DELAY_MS=20` (default) or `--hover-delay-ms 150` to tune, 0–2000 ms; `0` disables the highlight. The thicker outline needs one full redraw when the region changes. Its antialiasing now follows `g`, and the atlas strokes are composited once per pixel to prevent bright intersections. For precomputed wireframe outlines, thickening requires companion solid labels (`--atlas-labels`); without them, the outline is unchanged. A keyboard probe remains available with `I` if iOS Rex does not forward mouse-move events.

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

Select an image with its current stack number (`1` is top), or cycle with `;`. Press **`w`** to raise the selected image one position, or **`d`** to lower it (braces also work). The selection follows the moved image; its color map, opacity, display window, and visibility state remain attached to it. Press `t` for the top-to-bottom layer list (atlas remains above all images). Use ↑/↓ or `k`/`j` to select a layer without moving the slice. Press Enter, q or Esc to close the list, retaining the selected image; further Esc presses do nothing; use q or Ctrl+C to exit `niv`. The original base image can move above other intensity images, but the **atlas always stays on top**. Reordering does not copy or reload any image volume; it only changes compositing order and redraws the image. Newly loaded images are appended immediately below the atlas.

## Save and reload scenes (v0.15)

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
selected layer, inspection state, montage range/layout, and performance defaults.

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

### Projection views and layer visibility

Press `X` for maximum intensity projection or `A` for average intensity projection
along the current axis for the selected image. Other images default to MAX. Select
any layer and press X, A or U to change its own method; V returns to slice view.
Choices stay with the image when reordered and are retained during the session.
Projections use the full depth and the mean includes finite zeros; NaN and infinite
values are excluded. Each visible image is reduced before applying its display
window, palette, opacity and stacking order. Cached native-resolution 2D arrays
are reused for contrast changes, zoom, rotation, reordering and repeat visits to
the same axis and projection mode. No projection scans occur on ordinary startup.
Hidden images are not fully projected until displayed; their probe values are
computed individually if needed. The cache is in memory for this session only.

The atlas silhouette is the outline of the union of all nonzero finite labels
along the projection axis. In v0.20 it is one of the choices under `T`; grouped
contours are the default when General_Region is available. The silhouette uses
the configured single atlas outline color, width, opacity, AA and blur. A solid atlas or companion
solid label file is required. Toggle with `a`. Region-specific hover outlines are
disabled in projection view; `*` in the probe coordinates represents the collapsed
axis. Montage remains slice-based. Projection mode is not persisted in scenes.
Categorical images are reduced numerically too, so overlapping cluster IDs or
average IDs are not anatomically meaningful.

Space toggles any selected image, including the original reference image; it
remains loaded as the grid reference. With all images hidden, only the atlas
and background remain. Scene files save and restore the visibility of every image.

### v0.18.2 viewer refinements

- Leaving montage (`p`) restores the previously viewed slice if the axis is unchanged, or the middle slice otherwise (including a montage opened from a scene).
- The `n` file picker checks the highlighted file on Enter when nothing else is marked, and keeps its selection and loading progress visible until loading finishes.
- Hidden image entries and the selected-image footer are dimmed and retain the `[off]` marker.
- Display windows remain compact (five decimal places); `m`, `M`, and `e` accept higher-precision values and show the stored value(s) while editing. Values are stored as f32.
- Layout changes clear the previous footer even after a range or opacity edit that requested only a raster redraw.

### v0.18.3: categorical status and safe quit

Visible categorical (random LUT) filenames use a stable rainbow gradient in the
footer and layer list; numerical display settings remain neutral. Hidden
layers retain the dimmed text and `[off]` marker. This is text styling only.

`q` closes the active help/info popup, layer list or numeric editor before it
can quit the viewer. In file-search and scene-filename editors, `q` remains
a literal character; use Esc to cancel. Ctrl+C bypasses modal key handlers
and quits when the event loop can process it. Synchronous file loading may
need to finish first. Reported q repeat events are ignored outside text fields.

### v0.19: multi-image startup and per-image projection

`niv template.nii.gz clusters.nii.gz` and `niv -i template.nii.gz clusters.nii.gz`
are equivalent. The first image is the reference and top image (under the atlas); additional startup images are stacked below it in argument order. Quoted glob patterns expand in sorted order per
pattern; multiple patterns or repeated `-i` groups are accepted.

Select a layer and press X (maximum), A (average), or U (minimum positive). All visible images project
along the same axis, each using its own method, with MAX as the default for
other images. The layer list and selected-layer footer show [MAX], [AVG] or [MIN+].
V returns to slices. The dedicated projection help is available with ?.
Projection choices are retained in memory, but are not saved in scenes.
Categorical filenames now use one interpolated RGB color per character.

### v0.20: top-first startup order, projection outlines and shortcuts

- `niv -v` prints the version; CLI `--help` lists options, and `?` opens mode-specific keyboard help.
- Startup image arguments are top-to-bottom: layer 1 is the first image, under the fixed atlas. The original reference volume still defines the voxel grid. Reordering renumbers layers top-to-bottom and selection follows the moved image; `w` raises and `{` lowers. Newly loaded runtime images remain topmost beneath the atlas. Existing scenes retain their stored stack.
- `x`, `c` (or `y`), `z` select X, Y, Z axes. Repeating x/z mirrors the displayed image horizontally; max/mean/min projections are depth-order invariant, so this is not a different depth projection. `l` cycles the color map (no longer vim-right; use the right arrow).
- `u` or left click over a named atlas region requests clipboard copy of its abbreviation and name (OSC 52, subject to client permissions). Available in slice view only.
- `i` describes the selected image.
- `U` chooses the smallest finite positive intensity along a ray; an empty ray becomes zero. For positive size-ranked cluster IDs (1 = largest), MAX favors smaller clusters; MIN+ favors larger clusters. Neither resolves all overlaps: use multiple axes or montage to view all clusters.
- In projection mode `T` cycles major divisions (default), broad group, brain silhouette, and none. Every group is projected as an independent binary mask and its voxel-face outline cached on demand. Groups may overlap; coincident strokes use deterministic draw priority. Group color is the LUT color of its lowest-ID member, except cerebellum uses a single pale yellow. These are display groups, not new Allen labels. If General_Region is missing, the brain silhouette is used; if the atlas is only a wireframe, companion solid labels are required. Normal slices and startup perform no grouped projection work.
- Projection methods and outline detail are session-only; old scene files still load and maintain stored stack order.

### v0.21: image registers, independent orientation and simpler outlines

- `i` opens/closes information; `I` toggles keyboard probe vs mouse hover in slice mode.
- `Y` copies the selected relative path where possible via OSC 52 and remembers its window, palette, opacity, visibility and projection method. Open `t`, select an insertion point, and use `p` below or `P` above. A yank paste reloads the image as an independent layer; no file is copied or modified on disk.
- `X` in `t` marks a layer `[cut]` without unloading it. Navigate to the target and paste below/above; this moves the existing layer with no voxel copy. Closing the list cancels the pending move. Outside `t`, `X` still sets MAX. `A` and `U` still set projection methods in the list.
- `d` unloads the selected image. If the original reference is unloaded, another aligned image becomes the reference, retaining all layer settings and projection cache identity. The last image cannot be unloaded; Space can hide it for an atlas-only view.
- Each axis remembers its own rotation and flips for the current session. Scenes still store only the active axis transform.
- `T` in projection mode cycles major divisions, broad regions, whole-brain silhouette, none. Major mode combines selected anatomical divisions; both grouped modes merge all cerebellar cortex/nuclei and omit ventricular and fiber-tract outlines. Enclosed holes in projected masks are filled before drawing their exterior faces. Disconnected components and overlapping groups can still have distinct outlines. This is a presentation-only simplification, not a modification of the atlas or a surface rendering. Contiguous collinear edges are merged to reduce stroke work, and masks use a direct lookup for compact label IDs. Group preparation is lazy and cached by axis and detail.

### v0.22: opt-in projection atlas, compact numeric readouts, template and zoxide

- Projection outlines start **off**. `a` toggles them without changing slice-view atlas visibility; `T` enables or cycles major, broad, and silhouette styles. The chosen style and visibility are remembered for the current session. Grouped projections contain only their regional contours; no extra brain silhouette is drawn or computed. Missing regional metadata requires selecting the silhouette explicitly. Existing cached outlines remain reusable.
- Repeating `y` or its alias `c` flips the axial image horizontally, just like `x` and `z`. Axis transforms remain independent.
- Integer-valued probe intensities and selected-layer bounds are displayed without trailing zeros. Fractional probe intensities retain seven decimal places; window settings retain five where appropriate, with additional precision for near-integer thresholds. The editor and stored f32 values are unchanged.
- Set `TEMPLATE` to a registered grayscale volume in your sourced zsh configuration. `niv --template` opens it alone; `niv clusters.nii.gz --template` adds it as the bottom image. It is not loaded implicitly, and existing CLI and scene loading are unchanged.
- In the `n` file picker, Ctrl+Z opens the ranked zoxide directory browser (see below).

### v0.23: safe Escape and interactive directory preview

Escape closes the innermost open menu or editor; outside menus it does nothing.
Use `q` to quit from the normal viewer, or Ctrl+C from any input context.
The `q` character continues to be accepted in filename and directory search fields.

Press `n`, then Ctrl+Z to browse scored directories from `zoxide query --list --score`.
The ranking is loaded once per opening; type case-insensitive keywords to filter
paths, with substring matches preferred over fuzzy subsequence matches. Use
Up/Down or Tab/Shift+Tab to select, PgUp/PgDn to page, Ctrl+Up/Down to scroll
the selected directory's preview, Ctrl+U to clear the query, and Enter to jump.
Esc or Ctrl+Z returns to the original file filter and selection. Marked images
survive directory jumps. The preview lists visible entries from the selected
directory only, up to 256 entries, with no NIfTI decoding or recursive scans.
A plus sign in the entry count indicates a truncated preview. Zoxide is required
on the host running niv; fzf is not required. The browser does not modify cwd.
