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

/// Coarse CCF grouping for uncluttered projection figures. Other group names
/// remain available in broad mode; neither mode draws ventricular/fiber masks.
fn group_name(region: &str, detail: u8) -> Option<&str> {
    let top = region.split("__").next().unwrap_or("");
    if top.is_empty()
        || top.eq_ignore_ascii_case("Ventricular")
        || top.starts_with("Fiber_tract")
        || top.eq_ignore_ascii_case("root")
    {
        return None;
    }
    if top.starts_with("Cerebell") {
        return Some("Cerebellum");
    }
    if detail == 0 {
        match top {
            "Isocortex" | "Cortical_subplate" => Some("Cortex"),
            "Hippocampal" | "Retrohippocampal" => Some("Hippocampal formation"),
            "Striatum" | "Pallidum" => Some("Striatum / pallidum"),
            "Striatum-like" => Some("Striatum-like regions"),
            "Midbrain" | "Pons" | "Medulla" => Some("Brainstem"),
            "Thalamus" | "Hypothalamus" => Some(top),
            _ if top.starts_with("Olfactory") => Some("Olfactory areas"),
            _ if top.starts_with("Pallidum") => Some("Striatum / pallidum"),
            _ => None,
        }
    } else {
        Some(top)
    }
}

/// Fill enclosed background holes, retaining the exact exterior boundary.
/// This is a display-only simplification, not a modification of atlas labels.
fn fill_holes(mask: &mut [bool], w: usize, h: usize) {
    use std::collections::VecDeque;
    let mut exterior = vec![false; mask.len()];
    let mut queue = VecDeque::new();
    let enqueue = |idx: usize, exterior: &mut [bool], queue: &mut VecDeque<usize>| {
        if !mask[idx] && !exterior[idx] {
            exterior[idx] = true;
            queue.push_back(idx);
        }
    };
    for x in 0..w {
        enqueue(x, &mut exterior, &mut queue);
        enqueue((h - 1) * w + x, &mut exterior, &mut queue);
    }
    for y in 0..h {
        enqueue(y * w, &mut exterior, &mut queue);
        enqueue(y * w + w - 1, &mut exterior, &mut queue);
    }
    while let Some(i) = queue.pop_front() {
        let x = i % w;
        let y = i / w;
        if x > 0 {
            enqueue(i - 1, &mut exterior, &mut queue);
        }
        if x + 1 < w {
            enqueue(i + 1, &mut exterior, &mut queue);
        }
        if y > 0 {
            enqueue(i - w, &mut exterior, &mut queue);
        }
        if y + 1 < h {
            enqueue(i + w, &mut exterior, &mut queue);
        }
    }
    for (m, e) in mask.iter_mut().zip(exterior) {
        if !e {
            *m = true;
        }
    }
}

/// Merge contiguous collinear faces: fewer stroke calls, same geometry.
fn exterior_faces(mask: &[bool], w: usize, h: usize) -> Vec<Face> {
    let at = |x: usize, y: usize| mask[y * w + x];
    let mut faces = Vec::new();
    for x in 0..=w {
        let mut start = None;
        for y in 0..=h {
            let edge = y < h && ((x > 0 && at(x - 1, y)) != (x < w && at(x, y)));
            if edge {
                if start.is_none() {
                    start = Some(y);
                }
            } else if let Some(y0) = start.take() {
                faces.push(((x, y0), (x, y)));
            }
        }
    }
    for y in 0..=h {
        let mut start = None;
        for x in 0..=w {
            let edge = x < w && ((y > 0 && at(x, y - 1)) != (y < h && at(x, y)));
            if edge {
                if start.is_none() {
                    start = Some(x);
                }
            } else if let Some(x0) = start.take() {
                faces.push(((x0, y), (x, y)));
            }
        }
    }
    faces
}

impl ProjectionCache {
    /// Atlas changes do not invalidate cached intensity projections.
    pub fn clear_atlas(&self) {
        self.silhouettes.borrow_mut().clear();
        self.grouped.borrow_mut().clear();
    }
    pub fn remap_images(&self, remap: impl Fn(usize) -> Option<usize>) {
        self.planes.borrow_mut().retain_mut(|(id, _, _, _)| {
            if let Some(new) = remap(*id) {
                *id = new;
                true
            } else {
                false
            }
        });
    }
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

    /// Layer moves do not change IDs; unload remaps cached IDs explicitly.
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
            let Some(name) = group_name(&r.general_region, detail) else {
                continue;
            };
            let rgb = if name == "Cerebellum" {
                [255, 235, 110]
            } else {
                r.color
            };
            let entry = groups.entry(name.to_owned()).or_insert_with(|| {
                let i = colors.len();
                colors.push(rgb);
                (i, rgb)
            });
            lookup.insert(id, entry.0);
        }
        let (w, h, _) = solid.plane(axis);
        let mut masks = vec![vec![false; w * h]; colors.len()];
        let (a, b) = axis.plane_axes();
        // Compact atlases can use a direct lookup instead of hashing 45M voxels.
        let max_id = lookup.keys().copied().max().unwrap_or(0);
        let dense = if (0..=1_000_000).contains(&max_id) {
            let mut table = vec![usize::MAX; max_id as usize + 1];
            for (&id, &group) in &lookup {
                if id > 0 {
                    table[id as usize] = group;
                }
            }
            Some(table)
        } else {
            None
        };
        for ((x, y, z), &value) in solid.data.indexed_iter() {
            let id = crate::layers::label_id(value);
            if id <= 0 {
                continue;
            }
            let i = if let Some(table) = &dense {
                table.get(id as usize).copied().filter(|&i| i != usize::MAX)
            } else {
                lookup.get(&id).copied()
            };
            if let Some(i) = i {
                let xyz = [x, y, z];
                masks[i][xyz[b] * w + xyz[a]] = true;
            }
        }
        let mut faces = Vec::new();
        for (mut mask, color) in masks.into_iter().zip(colors) {
            fill_holes(&mut mask, w, h);
            faces.extend(
                exterior_faces(&mask, w, h)
                    .into_iter()
                    .map(|face| (face, color)),
            );
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
        let mut flat = (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .map(|(x, y)| mask[[x, y]])
            .collect::<Vec<_>>();
        fill_holes(&mut flat, w, h);
        let mask = |x: usize, y: usize| flat[y * w + x];
        let mut faces = Vec::new();
        for y in 0..h {
            for x in 0..=w {
                let left = x > 0 && mask(x - 1, y);
                let right = x < w && mask(x, y);
                if left != right {
                    faces.push(((x, y), (x, y + 1)));
                }
            }
        }
        for y in 0..=h {
            for x in 0..w {
                let low = y > 0 && mask(x, y - 1);
                let high = y < h && mask(x, y);
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
            spatial_units: 2,
            sform: None,
            datatype: 16,
        }
    }
    #[test]
    fn atlas_replacement_keeps_intensity_cache_but_drops_old_contours() {
        let vol = volume(Array3::from_elem((3, 3, 2), 1.));
        let empty = volume(Array3::zeros((3, 3, 2)));
        let cache = ProjectionCache::default();
        let plane = cache.plane(0, &vol, Axis::Z, ProjectionMode::Mean);
        assert!(!cache.silhouette(&vol, Axis::Z).is_empty());
        cache.clear_atlas();
        assert!(cache.grouped.borrow().is_empty());
        assert!(cache.silhouette(&empty, Axis::Z).is_empty());
        assert!(Rc::ptr_eq(
            &plane,
            &cache.plane(0, &vol, Axis::Z, ProjectionMode::Mean)
        ));
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
    fn projected_groups_merge_cerebellum_and_omit_ventricles() {
        use crate::layers::RegionInfo;
        use std::collections::HashMap;
        let vol = volume(Array3::from_shape_fn((3, 1, 2), |(x, _, _)| (x + 1) as f32));
        let mut regions = HashMap::new();
        for (id, group) in [
            (1, "Cerebellar_cortex__Vermis"),
            (2, "Cerebellar_nuclei"),
            (3, "Ventricular"),
        ] {
            regions.insert(
                id,
                RegionInfo {
                    color: [255, 0, 0],
                    name: group.into(),
                    abbr: group.into(),
                    general_region: group.into(),
                },
            );
        }
        let cache = ProjectionCache::default();
        for detail in [0, 1] {
            let faces = cache.grouped_faces(&vol, Axis::Z, detail, &regions);
            assert_eq!(faces.len(), 4);
            assert!(faces.iter().all(|(_, color)| *color == [255, 235, 110]));
            assert!(!faces.iter().any(|(face, _)| *face == ((1, 0), (1, 1))));
            assert!(Rc::ptr_eq(
                &faces,
                &cache.grouped_faces(&vol, Axis::Z, detail, &regions)
            ));
        }
        let mut ring = vec![true, true, true, true, false, true, true, true, true];
        fill_holes(&mut ring, 3, 3);
        assert!(ring.iter().all(|&x| x));
        assert_eq!(exterior_faces(&ring, 3, 3).len(), 4);
    }
    #[test]
    fn projection_cache_remaps_ids_after_unload() {
        let vol = volume(Array3::from_elem((2, 2, 2), 3.));
        let cache = ProjectionCache::default();
        let removed = cache.plane(0, &vol, Axis::Z, ProjectionMode::Max);
        let retained = cache.plane(1, &vol, Axis::Z, ProjectionMode::Max);
        cache.remap_images(|id| if id == 0 { None } else { Some(0) });
        assert!(Rc::ptr_eq(
            &retained,
            &cache.get_plane(0, Axis::Z, ProjectionMode::Max).unwrap()
        ));
        assert!(!Rc::ptr_eq(&removed, &retained));
        assert!(cache.get_plane(1, Axis::Z, ProjectionMode::Max).is_none());
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
