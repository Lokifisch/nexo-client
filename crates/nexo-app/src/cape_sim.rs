//! The cape tiles' motion: the same physics as Nexo Mod's capes (`NexoCapePhysics`),
//! cut down to a figure that stands still in moving air.
//!
//! The cape is 17 horizontal lines, one per texture row, each with its own momentum,
//! pulled down by gravity and along by the wind, kept one row apart. Only the side
//! view matters here (up, and out behind), so it is two-dimensional. Skin pixels and
//! seconds; behind the back is -z, as in `skin3d`.

pub const LINES: usize = 17;
const GRAVITY: f32 = 18.0 * 16.0;
/// Per second; the same as the mod's, so the cape lifts the same way at a given wind.
const DRAG: f32 = 3.5;
const STEP: f32 = 1.0 / 120.0;
const ITERATIONS: usize = 24;
/// Closest two lines one apart may come, as a share of two rows.
const FOLD: f32 = 0.45;

#[derive(Debug, Clone)]
pub struct CapeSim {
    y: [f32; LINES],
    z: [f32; LINES],
    py: [f32; LINES],
    pz: [f32; LINES],
    pending: f32,
    time: f32,
}

impl Default for CapeSim {
    /// Hanging straight down from the shoulders, at rest.
    fn default() -> Self {
        let y = std::array::from_fn(|i| -(i as f32));
        Self { y, z: [0.0; LINES], py: y, pz: [0.0; LINES], pending: 0.0, time: 0.0 }
    }
}

impl CapeSim {
    /// The air moving past the figure, in pixels per second toward its back:
    /// a walk's worth with gusts, like a player strolling into a breeze.
    pub fn breeze(time: f32) -> f32 {
        70.0 + 25.0 * (time * 0.9).sin() + 15.0 * (time * 2.3).sin()
    }

    pub fn advance(&mut self, seconds: f32, wind: f32) {
        self.pending = (self.pending + seconds).min(0.25);
        while self.pending >= STEP {
            self.pending -= STEP;
            self.step(wind);
        }
    }

    fn step(&mut self, wind: f32) {
        self.time += STEP;
        let keep = 1.0 - DRAG * STEP;
        for i in 1..LINES {
            let vy = (self.y[i] - self.py[i]) * keep;
            // Drag pulls toward the air's own speed, which blows toward -z.
            let vz = (self.z[i] - self.pz[i]) * keep - wind * DRAG * STEP * STEP;
            self.py[i] = self.y[i];
            self.pz[i] = self.z[i];
            // Flutter: faster air shakes each line along the cloth, as in the mod.
            let speed = (vy.hypot(vz + wind * STEP) / STEP).min(12.0 * 16.0);
            let shake = speed * 0.9 * (self.time * 14.0 + i as f32 * 0.8).sin() * STEP * STEP;
            self.y[i] += vy - GRAVITY * STEP * STEP;
            self.z[i] += vz - shake;
        }
        for _ in 0..ITERATIONS {
            for i in 1..LINES {
                self.hold(i - 1, i, 1.0, 1.0);
            }
            for i in 2..LINES {
                self.hold(i - 2, i, 2.0 * FOLD, f32::MAX);
            }
            // Never in front of the back while beside the body.
            for i in 1..LINES {
                if self.y[i] > -24.0 && self.z[i] > 0.0 {
                    self.z[i] = 0.0;
                }
            }
        }
    }

    fn hold(&mut self, a: usize, b: usize, min: f32, max: f32) {
        let (dy, dz) = (self.y[b] - self.y[a], self.z[b] - self.z[a]);
        let d = dy.hypot(dz);
        if d < 1e-6 {
            self.y[b] = self.y[a] - min;
            return;
        }
        let target = d.clamp(min, max);
        if target == d {
            return;
        }
        let push = (d - target) / d;
        // The top line is pinned to the shoulders.
        let share = if a == 0 { 1.0 } else { 0.5 };
        if a != 0 {
            self.y[a] += dy * push * share;
            self.z[a] += dz * push * share;
        }
        self.y[b] -= dy * push * share;
        self.z[b] -= dz * push * share;
    }

    /// Each strip's angle from hanging straight down, positive swinging out behind.
    pub fn angles(&self) -> [f32; LINES - 1] {
        std::array::from_fn(|i| (-(self.z[i + 1] - self.z[i])).atan2(-(self.y[i + 1] - self.y[i])))
    }

    /// Hanging still: nothing left to animate.
    pub fn settled(&self) -> bool {
        (1..LINES).all(|i| (self.y[i] - self.py[i]).abs() < 1e-3 && (self.z[i] - self.pz[i]).abs() < 1e-3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hangs_still_then_lifts_in_a_breeze_and_settles_again() {
        let mut cape = CapeSim::default();
        cape.advance(1.0, 0.0);
        assert!(cape.angles().iter().all(|a| a.abs() < 0.05), "hangs straight: {:?}", cape.angles());
        for _ in 0..120 {
            cape.advance(1.0 / 60.0, 90.0);
        }
        let hem = cape.angles()[LINES - 2].to_degrees();
        assert!((20.0..80.0).contains(&hem), "lifts out behind in a sprint's wind: {hem}");
        for _ in 0..600 {
            cape.advance(1.0 / 60.0, 0.0);
        }
        assert!(cape.settled(), "comes to rest when the wind stops");
    }
}
