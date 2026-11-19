//! Lazy, native-resolution intensity projections and grouped atlas outlines.
//! Only display modes that are actually requested allocate projected planes.
use crate::{
    app::ProjectionMode,
    volume::{Axis, Volume},
};
use ndarray::{Array2, ArrayView1, Axis as NdAxis};
use std::{cell::RefCell, rc::Rc};

pub type Face = ((usize, usize), (usize, usize));
type PlaneEntry = (usize, Axis, ProjectionMode, Rc<Array2<f32>>);
type SilhouetteEntry = (Axis, Rc<Vec<Face>>);
pub type ColoredFace = (Face, [u8; 3]);
type GroupEntry = (Axis, u8, Rc<Vec<ColoredFace>>);

#[derive(Default)]
pub struct ProjectionCache {
    planes: RefCell<Vec<PlaneEntry>>,
    silhouettes: RefCell<Vec<SilhouetteEntry>>,
    grouped: RefCell<Vec<GroupEntry>>,
}

fn reduce(ray: ArrayView1<'_, f32>, mode: ProjectionMode) -> f32 {
    match mode {
        ProjectionMode::Max => ray
            .iter()
            .copied()
            .filter(|v| v.is_finite())
            .reduce(f32::max)
            .unwrap_or(f32::NAN),
        ProjectionMode::MinPositive => ray
            .iter()
            .copied()
            .filter(|v| v.is_finite() && *v > 0.)
            .reduce(f32::min)
            .unwrap_or(0.),
        ProjectionMode::Mean => {
            let (sum, count) = ray
                .iter()
                .filter(|v| v.is_finite())
                .fold((0.0f64, 0usize), |(sum, n), &v| (sum + v as f64, n + 1));
            if count == 0 {
                f32::NAN
            } else {
                (sum / count as f64) as f32
            }
        }
        ProjectionMode::Slice => unreachable!("slice mode does not project"),
    }
}

impl ProjectionCache {
    pub fn get_plane(
        &self,
        id: usize,
        axis: Axis,
        mode: ProjectionMode,
    ) -> Option<Rc<Array2<f32>>> {
        self.planes
            .borrow()
            .iter()
            .find(|(i, a, m, _)| (*i, *a, *m) == (id, axis, mode))
            .map(|(_, _, _, p)| Rc::clone(p))
    }

    /// Image IDs are stable for the lifetime of the viewer: loading only appends
    /// new IDs, and reordering changes display positions, not image identities.
    pub fn plane(
        &self,
        id: usize,
        volume: &Volume,
        axis: Axis,
        mode: ProjectionMode,
    ) -> Rc<Array2<f32>> {
        if let Some(p) = self.get_plane(id, axis, mode) {
            return p;
        }
        let p = Rc::new(
            volume
                .data
                .map_axis(NdAxis(axis.index()), |ray| reduce(ray, mode)),
        );
        self.planes
            .borrow_mut()
            .push((id, axis, mode, Rc::clone(&p)));
        p
    }

    /// Inspect an undisplayed layer without computing its entire projection.
    pub fn value(
        &self,
        id: usize,
        volume: &Volume,
        axis: Axis,
        mode: ProjectionMode,
        h: usize,
        v: usize,
    ) -> f32 {
        if let Some(p) = self.get_plane(id, axis, mode) {
            return p[[h, v]];
        }
        use ndarray::s;
        let ray = match axis {
            Axis::X => volume.data.slice(s![.., h, v]),
            Axis::Y => volume.data.slice(s![h, .., v]),
            Axis::Z => volume.data.slice(s![h, v, ..]),
        };
        reduce(ray, mode)
    }

    /// Project each anatomical group independently: a ray can belong to
    /// several groups. Cache native voxel-face contours (not display pixels).
    pub fn grouped_faces(
        &self,
        solid: &Volume,
        axis: Axis,
        detail: u8,
        regions: &std::collections::HashMap<i32, crate::layers::RegionInfo>,
    ) -> Rc<Vec<ColoredFace>> {
        if let Some((_, _, faces)) = self
            .grouped
            .borrow()
            .iter()
            .find(|(a, d, _)| *a == axis && *d == detail)
        {
            return Rc::clone(faces);
        }
        use std::collections::{BTreeMap, HashMap};
        let mut groups: BTreeMap<String, (usize, [u8; 3])> = BTreeMap::new();
        let mut ids: Vec<_> = regions.keys().copied().collect();
        ids.sort_unstable();
        let mut lookup = HashMap::new();
        let mut colors = Vec::new();
        for id in ids {
            let r = &regions[&id];
            if r.general_region.is_empty() {
                continue;
            }
            let name = if detail == 1 {
                r.general_region.split("__").next().unwrap_or("")
            } else {
                r.general_region.as_str()
            };
            let entry = groups.entry(name.to_owned()).or_insert_with(|| {
                let i = colors.len();
                colors.push(r.color);
                (i, r.color)
            });
            lookup.insert(id, entry.0);
        }
        let (w, h, _) = solid.plane(axis);
        let mut masks = vec![vec![false; w * h]; colors.len()];
        let (a, b) = axis.plane_axes();
        for ((x, y, z), &value) in solid.data.indexed_iter() {
            if let Some(&i) = lookup.get(&crate::layers::label_id(value)) {
                let xyz = [x, y, z];
                masks[i][xyz[b] * w + xyz[a]] = true;
            }
        }
        let mut faces = Vec::new();
        for (mask, color) in masks.iter().zip(colors) {
            let at = |x: usize, y: usize| mask[y * w + x];
            for y in 0..h {
                for x in 0..=w {
                    if (x > 0 && at(x - 1, y)) != (x < w && at(x, y)) {
                        faces.push((((x, y), (x, y + 1)), color));
                    }
                }
            }
            for y in 0..=h {
                for x in 0..w {
                    if (y > 0 && at(x, y - 1)) != (y < h && at(x, y)) {
                        faces.push((((x, y), (x + 1, y)), color));
                    }
                }
            }
        }
        let faces = Rc::new(faces);
        self.grouped
            .borrow_mut()
            .push((axis, detail, Rc::clone(&faces)));
        faces
    }

    /// Union of all nonzero finite atlas labels over the chosen depth axis.
    /// Cache boundary faces, not a raster, so zoom/AA/blur remain adjustable.
    pub fn silhouette(&self, solid: &Volume, axis: Axis) -> Rc<Vec<Face>> {
        if let Some((_, faces)) = self.silhouettes.borrow().iter().find(|(a, _)| *a == axis) {
            return Rc::clone(faces);
        }
        let mask = solid.data.map_axis(NdAxis(axis.index()), |ray| {
            ray.iter().any(|v| v.is_finite() && *v != 0.0)
        });
        let (w, h) = mask.dim();
        let mut faces = Vec::new();
        for y in 0..h {
            for x in 0..=w {
                let left = x > 0 && mask[[x - 1, y]];
                let right = x < w && mask[[x, y]];
                if left != right {
                    faces.push(((x, y), (x, y + 1)));
                }
            }
        }
        for y in 0..=h {
            for x in 0..w {
                let low = y > 0 && mask[[x, y - 1]];
                let high = y < h && mask[[x, y]];
                if low != high {
                    faces.push(((x, y), (x + 1, y)));
                }
            }
        }
        let faces = Rc::new(faces);
        self.silhouettes
            .borrow_mut()
            .push((axis, Rc::clone(&faces)));
        faces
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ndarray::Array3;
    fn volume(data: Array3<f32>) -> Volume {
        Volume {
            data,
            spacing: [1.; 3],
            sform: None,
            datatype: 16,
        }
    }
    #[test]
    fn planes_are_lazy_and_reused_by_axis_mode_and_id() {
        let vol = volume(Array3::from_shape_fn((2, 3, 4), |(x, y, z)| {
            (x * 100 + y * 10 + z) as f32
        }));
        let c = ProjectionCache::default();
        assert!(c.planes.borrow().is_empty());
        for axis in [Axis::X, Axis::Y, Axis::Z] {
            let (w, h, _) = vol.plane(axis);
            for mode in [ProjectionMode::Max, ProjectionMode::Mean] {
                let p = c.plane(0, &vol, axis, mode);
                assert_eq!(p.dim(), (w, h));
                for v in 0..h {
                    for u in 0..w {
                        let values: Vec<_> = (0..vol.depth(axis))
                            .map(|i| vol.sample(axis, i, u, v))
                            .collect();
                        let expected = if mode == ProjectionMode::Max {
                            values.into_iter().fold(f32::NEG_INFINITY, f32::max)
                        } else {
                            values.iter().sum::<f32>() / values.len() as f32
                        };
                        assert_eq!(p[[u, v]], expected);
                    }
                }
                assert!(Rc::ptr_eq(&p, &c.plane(0, &vol, axis, mode)));
            }
        }
        let other = volume(Array3::from_elem((2, 3, 4), -5.));
        assert_eq!(
            c.plane(1, &other, Axis::Z, ProjectionMode::Max)[[0, 0]],
            -5.
        );
    }
    #[test]
    fn mean_includes_zeros_and_ignores_nonfinite() {
        let vol = volume(
            Array3::from_shape_vec(
                (2, 1, 3),
                vec![0., 6., f32::NAN, f32::NAN, f32::INFINITY, f32::NAN],
            )
            .unwrap(),
        );
        let c = ProjectionCache::default();
        assert_eq!(c.value(0, &vol, Axis::Z, ProjectionMode::Mean, 0, 0), 3.);
        assert!(c.planes.borrow().is_empty());
        let mean = c.plane(0, &vol, Axis::Z, ProjectionMode::Mean);
        assert_eq!(mean[[0, 0]], 3.);
        assert!(mean[[1, 0]].is_nan());
        assert_eq!(c.plane(0, &vol, Axis::Z, ProjectionMode::Max)[[0, 0]], 6.);
    }
    #[test]
    fn minimum_positive_ignores_zero_negative_and_nonfinite() {
        let vol = volume(
            Array3::from_shape_vec((2, 1, 4), vec![0., 8., 2., f32::NAN, 0., -1., f32::NAN, 0.])
                .unwrap(),
        );
        let cache = ProjectionCache::default();
        let p = cache.plane(0, &vol, Axis::Z, ProjectionMode::MinPositive);
        assert_eq!(p[[0, 0]], 2.);
        assert_eq!(p[[1, 0]], 0.);
        assert_eq!(
            cache.plane(0, &vol, Axis::Z, ProjectionMode::Max)[[0, 0]],
            8.
        );
    }
    #[test]
    fn projected_groups_keep_overlaps_and_merge_subgroups_at_broad_level() {
        use crate::layers::RegionInfo;
        use std::collections::HashMap;
        let vol = volume(Array3::from_shape_fn((2, 1, 2), |(x, _, z)| {
            if x == 0 && z == 0 {
                1.
            } else if z == 1 {
                2.
            } else {
                0.
            }
        }));
        let mut regions = HashMap::new();
        for (id, group, color) in [
            (1, "Striatum__Dorsal", [255, 0, 0]),
            (2, "Striatum__Ventral", [0, 255, 0]),
        ] {
            regions.insert(
                id,
                RegionInfo {
                    color,
                    name: group.into(),
                    abbr: group.into(),
                    general_region: group.into(),
                },
            );
        }
        let cache = ProjectionCache::default();
        let detailed = cache.grouped_faces(&vol, Axis::Z, 0, &regions);
        assert!(detailed.contains(&(((1, 0), (1, 1)), [255, 0, 0])));
        assert!(detailed.iter().any(|(_, color)| *color == [0, 255, 0]));
        let broad = cache.grouped_faces(&vol, Axis::Z, 1, &regions);
        assert_eq!(broad.len(), 6);
        assert!(!broad.iter().any(|(face, _)| *face == ((1, 0), (1, 1))));
        assert!(Rc::ptr_eq(
            &broad,
            &cache.grouped_faces(&vol, Axis::Z, 1, &regions)
        ));
    }
    #[test]
    fn binary_union_outline_has_no_internal_region_boundary() {
        let vol = volume(Array3::from_shape_fn((2, 1, 2), |(x, _, z)| {
            if x == z {
                (x + 3) as f32
            } else {
                0.
            }
        }));
        let c = ProjectionCache::default();
        let edges = c.silhouette(&vol, Axis::Z);
        assert_eq!(edges.len(), 6);
        assert!(!edges.contains(&((1, 0), (1, 1))));
        assert!(Rc::ptr_eq(&edges, &c.silhouette(&vol, Axis::Z)));
    }
}
