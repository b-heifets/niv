//! Lazy, native-resolution intensity projections and binary atlas silhouettes.
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

#[derive(Default)]
pub struct ProjectionCache {
    planes: RefCell<Vec<PlaneEntry>>,
    silhouettes: RefCell<Vec<SilhouetteEntry>>,
}

fn reduce(ray: ArrayView1<'_, f32>, mode: ProjectionMode) -> f32 {
    match mode {
        ProjectionMode::Max => ray
            .iter()
            .copied()
            .filter(|v| v.is_finite())
            .reduce(f32::max)
            .unwrap_or(f32::NAN),
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
