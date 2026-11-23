# niv v0.29.0 — experimental edit-mode integration

Changes from v0.28.0:

- **Ctrl+E** opens a non-destructive editing session from normal viewing using the loaded reference NIfTI, retaining the selected axis and slice; `q` returns to the viewer.
- `niv edit IMAGE.nii.gz` no longer requires `--mask`. `--masked` remains supported.
- No initial segmentation is assumed. Press `n` for Trim or `N` for Fill; enter a name or press Enter for the generated name.
- The first Trim segmentation is green. Segment opacity defaults to 22%. A colored footer and the `l` segment-list menu permit selection and visibility toggles. Saved visibility is restored from TOML sidecars.
- `z/x/c` focus planes; `Z/X/C` show/hide individual planes, `1/2/3` and Tab work; `f` switches to/from a single pane.
- `[` / `]` adjust slice stride, `,` / `.` or Shift+wheel adjust the 3D brush diameter.
- Ctrl+wheel zooms only the selected pane; Shift+arrow and Shift+left-drag pan it. Shift+hover updates the orthogonal crosshairs without painting.
- `u` and Cmd+Z undo; `U` and Ctrl+R redo; `?` and Shift+/ open in-editor help.
- `B` toggles optional binary brain mask export (`>0` source threshold when no mask is provided), as does `--binarize-mask` when launching directly.
- The old trim/fill and undo model is unchanged, including saving segmentation `.nii.gz` plus `.toml` sidecars.

Build on the existing checkout:

```sh
cd ~/Documents/_GitHub/niv
unzip -o ~/Downloads/niv-v0.29.0-edit-mode-update.zip -d .
cargo fmt && cargo test && cargo build --release
./target/release/niv -v
```

**Testing:** Source/static syntax checks performed when packaged. Rust compiler and interactive Kitty/Ghostty terminal were unavailable; compilation and interactive behavior must be confirmed on the target system.
