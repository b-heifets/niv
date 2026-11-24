# niv v0.30.0 — editor interaction update (experimental)

- Opening `niv` without input now opens the searchable NIfTI picker.
- `Ctrl+E` edits the selected image in normal viewing and imports its display window and slice.
- The editor supports loading further aligned NIfTI images via `n`, selecting a displayed image in a rounded bottom image bar, and adjusting image-specific grayscale ranges with `m/M/w`.
- Segmentations are generic, named, and default to *no intensity effect*. `t` cycles off / inside / outside, while `I` controls target intensity (zero for trimming); saved metadata retains the operation. Older Trim/Fill sidecars remain supported.
- While painting, the original grayscale image is displayed beneath translucent segmentation overlays. `p` optionally previews the edited image.
- The brush outline appears in all three views. Precise Shift-hover synchronizes the crosshair, and middle-button drag or Shift+drag pans.
- Per-view zoom now changes by 0.1×, supports reset with `0`, and retains independent rotation (`r`) and flips (`h/v`). Arrow panning and linked-plane movement respect stride.
- `Tab` focuses another plane; `z/x/c/y` toggles view visibility; `f`/Enter shows only the focused plane or restores all three.
- `1`–`9` select segments, Shift+number toggles visibility, `;` cycles, and `a` creates segments. Undo and redo now include Ctrl/Command combinations.
- The help/info/segmentation menus use centered, sectioned panels, and editor shortcuts no longer occupy the bottom status row.

**Build status:** Source update only. The Rust compiler is unavailable in this environment; validate with `cargo fmt && cargo test && cargo build --release` before committing or using edited images.
