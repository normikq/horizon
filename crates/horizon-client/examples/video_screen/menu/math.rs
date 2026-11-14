//! The menu's 3D geometry: where its panels stand in the world, and where the
//! crosshair's ray meets them.
//!
//! Every panel is a flat rectangle laid out in its own 2D units (`x` right,
//! `y` down, origin at the top-left corner), `UNIT` blocks per unit. In the
//! world it stands upright, turned about the vertical axis so that it faces
//! the point the menu was opened from.

/// Blocks per layout unit: 128 units make one block, and a line of the
/// game's 9-pixel font (drawn one font pixel per unit) is 0.07 block tall.
pub(crate) const UNIT: f64 = 1.0 / 128.0;

/// How far in front of the eyes the menu opens, in blocks.
pub(crate) const MENU_DISTANCE: f64 = 2.4;

/// The closest the menu opens, in blocks, when a block stands nearer.
pub(crate) const MIN_DISTANCE: f64 = 0.8;

/// The steepest the menu follows the view up or down when it opens.
const MAX_PITCH: f64 = 25.0;

pub(crate) type Vec3 = [f64; 3];

pub(crate) fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
pub(crate) fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub(crate) fn scale(a: Vec3, s: f64) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
pub(crate) fn dot(a: Vec3, b: Vec3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The horizontal direction the game's `yaw` looks along (0 is +Z, 90 is -X).
pub(crate) fn yaw_forward(yaw_degrees: f64) -> Vec3 {
    let (sin, cos) = yaw_degrees.to_radians().sin_cos();
    [-sin, 0.0, cos]
}

/// The direction the camera looks along, `pitch` positive looking down.
pub(crate) fn look_dir(yaw_degrees: f64, pitch_degrees: f64) -> Vec3 {
    let (sin_p, cos_p) = pitch_degrees.to_radians().sin_cos();
    let f = yaw_forward(yaw_degrees);
    [f[0] * cos_p, -sin_p, f[2] * cos_p]
}

/// One upright panel in the world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PanelPose {
    /// The world position of the layout's (0, 0), its top-left corner.
    pub origin: Vec3,
    /// The world direction of the layout's +x, horizontal and unit length.
    pub right: Vec3,
    /// The panel's size in layout units.
    pub size: [f64; 2],
    /// The panel's turn about +Y in degrees, as `BlockEntityDraw::rotate_about`
    /// takes it, so the pose's +X lies along `right` and +Z faces the viewer.
    pub turn_degrees: f32,
    /// Blocks per layout unit: `UNIT`, less for a menu opened close up.
    pub unit: f64,
}

impl PanelPose {
    /// A panel of `size` units centred on `center`, facing back along
    /// `facing` (the direction the viewer looks at it with).
    #[cfg(test)]
    pub(crate) fn facing(center: Vec3, facing_yaw: f64, size: [f64; 2]) -> Self {
        Self::facing_scaled(center, facing_yaw, size, UNIT)
    }

    /// As `facing`, `unit` blocks to a layout unit.
    pub(crate) fn facing_scaled(center: Vec3, facing_yaw: f64, size: [f64; 2], unit: f64) -> Self {
        let f = yaw_forward(facing_yaw);
        // The viewer's right hand: forward turned a quarter clockwise seen from above.
        let right = [-f[2], 0.0, f[0]];
        let half = [size[0] * unit * 0.5, size[1] * unit * 0.5];
        let origin = [
            center[0] - right[0] * half[0],
            center[1] + half[1],
            center[2] - right[2] * half[0],
        ];
        // A right-handed turn of θ about +Y takes +X to (cos θ, 0, -sin θ).
        let turn = (-right[2]).atan2(right[0]).to_degrees() as f32;
        Self {
            origin,
            right,
            size,
            turn_degrees: turn,
            unit,
        }
    }

    /// The unit normal pointing to the viewer's side.
    pub(crate) fn normal(&self) -> Vec3 {
        // right × up, with up = +Y: (r.x, 0, r.z) × (0, 1, 0) = (-r.z, 0, r.x)
        [-self.right[2], 0.0, self.right[0]]
    }

    /// The world position of the layout point (`x`, `y`), `z` units in front.
    pub(crate) fn to_world(&self, x: f64, y: f64, z: f64) -> Vec3 {
        add(
            add(self.origin, scale(self.right, x * self.unit)),
            add([0.0, -y * self.unit, 0.0], scale(self.normal(), z * self.unit)),
        )
    }

    /// Where the ray from `eye` along `dir` meets the panel, in layout units,
    /// with the distance along the ray; None if it misses or comes from behind.
    pub(crate) fn raycast(&self, eye: Vec3, dir: Vec3) -> Option<([f64; 2], f64)> {
        let ([x, y], t) = self.project(eye, dir)?;
        (x >= 0.0 && y >= 0.0 && x <= self.size[0] && y <= self.size[1]).then_some(([x, y], t))
    }

    /// Where the ray meets the panel's whole plane, inside the panel or not:
    /// a slider held down follows the crosshair past the panel's edge.
    pub(crate) fn project(&self, eye: Vec3, dir: Vec3) -> Option<([f64; 2], f64)> {
        let n = self.normal();
        let denom = dot(dir, n);
        // Only from the front: the ray must run against the normal.
        if denom > -1e-6 {
            return None;
        }
        let t = dot(sub(self.origin, eye), n) / denom;
        if !(t > 0.0) {
            return None;
        }
        let rel = sub(add(eye, scale(dir, t)), self.origin);
        let x = dot(rel, self.right) / self.unit;
        let y = -rel[1] / self.unit;
        Some(([x, y], t))
    }
}

/// Where the menu stands: fixed when it opens, so the view turns freely over it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MenuAnchor {
    /// The centre of the menu, between its panels.
    pub origin: Vec3,
    /// The yaw the menu was opened looking along.
    pub yaw: f32,
    /// The pitch it was opened with, clamped.
    pub pitch: f32,
    /// How large it is, 1 at `MENU_DISTANCE`: a menu opened closer is smaller
    /// in proportion, so it looks the same size.
    pub scale: f64,
}

impl MenuAnchor {
    /// The anchor `MENU_DISTANCE` in front of `eye`, looking along `yaw`/`pitch`.
    #[cfg(test)]
    pub(crate) fn in_front(eye: Vec3, yaw: f32, pitch: f32) -> Self {
        Self::at_distance(eye, yaw, pitch, MENU_DISTANCE)
    }

    /// The anchor `distance` blocks in front of `eye` (at most `MENU_DISTANCE`,
    /// at least `MIN_DISTANCE`), scaled to look as it does at `MENU_DISTANCE`.
    pub(crate) fn at_distance(eye: Vec3, yaw: f32, pitch: f32, distance: f64) -> Self {
        let distance = distance.clamp(MIN_DISTANCE, MENU_DISTANCE);
        let pitch = f64::from(pitch).clamp(-MAX_PITCH, MAX_PITCH);
        let dir = look_dir(f64::from(yaw), pitch);
        Self {
            origin: add(eye, scale(dir, distance)),
            yaw,
            pitch: pitch as f32,
            scale: distance / MENU_DISTANCE,
        }
    }

    /// A panel of `size` units whose centre is `offset` blocks to the right of
    /// the anchor and `drop` blocks below it, turned `toe_in` degrees to face
    /// the eye (positive turns its outer edge toward the viewer).
    pub(crate) fn panel(&self, offset: f64, drop: f64, size: [f64; 2], toe_in: f64) -> PanelPose {
        let (offset, drop) = (offset * self.scale, drop * self.scale);
        let f = yaw_forward(f64::from(self.yaw));
        let right = [-f[2], 0.0, f[0]];
        let center = [
            self.origin[0] + right[0] * offset,
            self.origin[1] - drop,
            self.origin[2] + right[2] * offset,
        ];
        // A panel to the right turns its face leftward, toward the eye.
        let facing = f64::from(self.yaw) + toe_in * offset.signum();
        // Push panels to the sides a little back so their inner edges meet the centre.
        let back = offset.abs() * (toe_in.to_radians().sin() * 0.5);
        let center = add(center, scale(f, back));
        PanelPose::facing_scaled(center, facing, size, UNIT * self.scale)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn yaw_zero_looks_along_plus_z() {
        let f = yaw_forward(0.0);
        assert!(close(f[0], 0.0) && close(f[2], 1.0));
        let f = yaw_forward(90.0);
        assert!(close(f[0], -1.0) && close(f[2], 0.0));
    }

    #[test]
    fn a_panel_faces_whoever_looks_at_it() {
        let p = PanelPose::facing([0.0, 0.0, 2.0], 0.0, [128.0, 128.0]);
        // Facing +Z means its normal points back to -Z, the viewer's side.
        let n = p.normal();
        assert!(close(n[2], -1.0), "{n:?}");
        // The viewer's right at yaw 0 is -X.
        assert!(close(p.right[0], -1.0));
        // The top-left corner is up and to the viewer's left.
        assert!(close(p.origin[0], 0.5) && close(p.origin[1], 0.5));
    }

    #[test]
    fn the_crosshair_through_the_centre_hits_the_middle() {
        let p = PanelPose::facing([0.0, 1.0, 2.0], 0.0, [100.0, 60.0]);
        let (uv, t) = p.raycast([0.0, 1.0, 0.0], [0.0, 0.0, 1.0]).unwrap();
        assert!(close(uv[0], 50.0) && close(uv[1], 30.0), "{uv:?}");
        assert!(close(t, 2.0));
    }

    #[test]
    fn the_layout_maps_back_through_to_world() {
        let p = PanelPose::facing([3.0, 64.0, -7.0], 37.0, [200.0, 150.0]);
        let point = p.to_world(40.0, 90.0, 0.0);
        let eye = add(point, scale(p.normal(), 2.0));
        let dir = scale(p.normal(), -1.0);
        let (uv, _) = p.raycast(eye, dir).unwrap();
        assert!(close(uv[0], 40.0) && close(uv[1], 90.0), "{uv:?}");
    }

    #[test]
    fn rays_from_behind_or_beside_miss() {
        let p = PanelPose::facing([0.0, 0.0, 2.0], 0.0, [128.0, 128.0]);
        assert!(p.raycast([0.0, 0.0, 4.0], [0.0, 0.0, -1.0]).is_none());
        assert!(p.raycast([0.0, 0.0, 0.0], [1.0, 0.0, 0.0]).is_none());
        assert!(p.raycast([5.0, 0.0, 0.0], [0.0, 0.0, 1.0]).is_none());
    }

    #[test]
    fn the_turn_carries_pose_x_onto_right() {
        for yaw in [0.0, 45.0, 90.0, 180.0, -120.0] {
            let p = PanelPose::facing([0.0; 3], yaw, [10.0, 10.0]);
            let (s, c) = f64::from(p.turn_degrees).to_radians().sin_cos();
            assert!(close(c, p.right[0]) && close(-s, p.right[2]), "yaw {yaw}");
        }
    }

    #[test]
    fn the_anchor_opens_in_front_of_the_eye() {
        let a = MenuAnchor::in_front([0.0, 70.0, 0.0], 0.0, 0.0);
        assert!(close(a.origin[2], MENU_DISTANCE) && close(a.origin[1], 70.0));
        let steep = MenuAnchor::in_front([0.0, 70.0, 0.0], 0.0, 89.0);
        assert!(close(f64::from(steep.pitch), 25.0));
    }

    #[test]
    fn a_closer_menu_is_smaller_in_proportion() {
        let eye = [0.0, 70.0, 0.0];
        let far = MenuAnchor::in_front(eye, 0.0, 0.0);
        let near = MenuAnchor::at_distance(eye, 0.0, 0.0, MENU_DISTANCE / 2.0);
        assert!(close(near.scale, 0.5) && close(near.origin[2], MENU_DISTANCE / 2.0));
        // The same layout point is seen in the same direction from the eye.
        for offset in [-1.0, 1.0] {
            let a = far.panel(offset, 0.0, [100.0, 80.0], 18.0).to_world(10.0, 20.0, 0.0);
            let b = near.panel(offset, 0.0, [100.0, 80.0], 18.0).to_world(10.0, 20.0, 0.0);
            let (da, db) = (sub(a, eye), sub(b, eye));
            let cos = dot(da, db) / (dot(da, da).sqrt() * dot(db, db).sqrt());
            assert!(cos > 0.999, "{cos}");
        }
        // And never closer than the least distance.
        let wall = MenuAnchor::at_distance(eye, 0.0, 0.0, 0.1);
        assert!(close(wall.origin[2], MIN_DISTANCE));
    }

    #[test]
    fn side_panels_turn_toward_the_centre() {
        let a = MenuAnchor::in_front([0.0, 70.0, 0.0], 0.0, 0.0);
        let left = a.panel(-1.0, 0.0, [100.0, 100.0], 20.0);
        let right = a.panel(1.0, 0.0, [100.0, 100.0], 20.0);
        // Both still face the eye: a ray from the eye to each centre hits it.
        for p in [left, right] {
            let c = p.to_world(50.0, 50.0, 0.0);
            let d = sub(c, [0.0, 70.0, 0.0]);
            let len = dot(d, d).sqrt();
            let hit = p.raycast([0.0, 70.0, 0.0], scale(d, 1.0 / len)).unwrap();
            assert!(close(hit.0[0], 50.0) && close(hit.0[1], 50.0));
        }
        // And their normals lean inward, toward the eye's side.
        assert!(left.normal()[0] < 0.0 && right.normal()[0] > 0.0);
    }
}
