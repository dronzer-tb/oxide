//! Fast Frustum & Axis-Aligned Bounding Box (AABB) Occlusion Culler.

#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct Plane {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
}

#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct Frustum {
    pub planes: [Plane; 6],
}

#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct SectionAABB {
    pub min_x: f32,
    pub min_y: f32,
    pub min_z: f32,
    pub max_x: f32,
    pub max_y: f32,
    pub max_z: f32,
}

impl Frustum {
    #[inline(always)]
    pub fn test_aabb(&self, aabb: &SectionAABB) -> bool {
        for plane in &self.planes {
            // Find positive vertex along plane normal
            let px = if plane.a > 0.0 { aabb.max_x } else { aabb.min_x };
            let py = if plane.b > 0.0 { aabb.max_y } else { aabb.min_y };
            let pz = if plane.c > 0.0 { aabb.max_z } else { aabb.min_z };

            if plane.a * px + plane.b * py + plane.c * pz + plane.d < 0.0 {
                return false; // Outside frustum
            }
        }
        true // Inside or intersecting
    }
}

/// Batch culls an array of section AABBs against the view frustum.
/// Returns bitmask or list of visible indices.
pub fn cull_sections(frustum: &Frustum, aabbs: &[SectionAABB], visible_out: &mut [u8]) -> usize {
    let mut visible_count = 0;
    for (i, aabb) in aabbs.iter().enumerate() {
        if frustum.test_aabb(aabb) {
            visible_out[i] = 1;
            visible_count += 1;
        } else {
            visible_out[i] = 0;
        }
    }
    visible_count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_frustum_inside_and_outside() {
        // Orthographic bounding box [0, 0, 0] to [100, 100, 100]
        let frustum = Frustum {
            planes: [
                Plane { a: 1.0, b: 0.0, c: 0.0, d: 0.0 },     // X >= 0
                Plane { a: -1.0, b: 0.0, c: 0.0, d: 100.0 },  // X <= 100
                Plane { a: 0.0, b: 1.0, c: 0.0, d: 0.0 },     // Y >= 0
                Plane { a: 0.0, b: -1.0, c: 0.0, d: 100.0 },  // Y <= 100
                Plane { a: 0.0, b: 0.0, c: 1.0, d: 0.0 },     // Z >= 0
                Plane { a: 0.0, b: 0.0, c: -1.0, d: 100.0 },  // Z <= 100
            ],
        };

        let inside = SectionAABB {
            min_x: 10.0, min_y: 10.0, min_z: 10.0,
            max_x: 26.0, max_y: 26.0, max_z: 26.0,
        };
        assert!(frustum.test_aabb(&inside));

        let outside = SectionAABB {
            min_x: 150.0, min_y: 10.0, min_z: 10.0,
            max_x: 166.0, max_y: 26.0, max_z: 26.0,
        };
        assert!(!frustum.test_aabb(&outside));
    }
}
