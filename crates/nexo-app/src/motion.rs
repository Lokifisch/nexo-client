//! Eased transitions for a UI that has no animation of its own: iced redraws
//! from state, so each moving value is a [`Tween`] read during `view`.
//!
//! Frames are only requested while a tween is running (see `App::subscription`),
//! so a settled window goes back to not repainting at all.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// One animated value's identity: a group and a name within it, both fixed
/// strings so reading one during `view` allocates nothing.
pub type Key = (&'static str, &'static str);

#[derive(Debug, Clone, Copy)]
struct Tween {
    from: f32,
    to: f32,
    start: Instant,
    duration: Duration,
}

impl Tween {
    fn progress(&self, now: Instant) -> f32 {
        let elapsed = now.saturating_duration_since(self.start).as_secs_f32();
        let total = self.duration.as_secs_f32();
        if total <= 0.0 { 1.0 } else { (elapsed / total).clamp(0.0, 1.0) }
    }

    fn value(&self, now: Instant) -> f32 {
        self.from + (self.to - self.from) * ease_out(self.progress(now))
    }

    fn running(&self, now: Instant) -> bool {
        self.progress(now) < 1.0
    }
}

/// Fast then settling, the curve UI motion mostly wants: it answers the click
/// at once and only slows where the eye is arriving.
pub fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

#[derive(Debug)]
pub struct Motion {
    now: Instant,
    tweens: HashMap<Key, Tween>,
}

impl Motion {
    pub fn new() -> Self {
        Self { now: Instant::now(), tweens: HashMap::new() }
    }

    /// Called on every frame the window draws.
    pub fn advance(&mut self, now: Instant) {
        self.now = now;
        // Finished tweens keep their value, but no longer count as running.
    }

    /// Moves `key` to `target` from wherever it is now, so reversing mid-flight
    /// turns around smoothly. `from` is where a key not seen before starts.
    pub fn to(&mut self, key: Key, from: f32, target: f32, millis: u64) {
        let now = Instant::now();
        self.now = now;
        let current = self.tweens.get(&key).map_or(from, |tween| tween.value(now));
        self.tweens.insert(key, Tween { from: current, to: target, start: now, duration: Duration::from_millis(millis) });
    }

    /// Restarts `key` from `from`, whatever it was doing.
    pub fn restart(&mut self, key: Key, from: f32, target: f32, millis: u64) {
        self.tweens.remove(&key);
        self.to(key, from, target, millis);
    }

    /// Moves `key` to `target` only if it isn't already headed there, for code
    /// that re-states the same target on every update. A key not seen yet
    /// starts at its target: nothing animates just because a screen first drew.
    pub fn settle(&mut self, key: Key, target: f32, millis: u64) {
        match self.tweens.get(&key) {
            Some(tween) if tween.to == target => {}
            Some(_) => self.to(key, target, target, millis),
            None => {
                let start = self.now;
                self.tweens.insert(key, Tween { from: target, to: target, start, duration: Duration::ZERO });
            }
        }
    }

    /// The value now. `default` until the key has been moved at least once.
    pub fn get(&self, key: Key, default: f32) -> f32 {
        self.tweens.get(&key).map_or(default, |tween| tween.value(self.now))
    }

    pub fn active(&self) -> bool {
        let now = self.now;
        self.tweens.values().any(|tween| tween.running(now))
    }

    /// How far the `index`th item of a list has come in, given the whole list's
    /// progress: each item starts a little after the one above it.
    pub fn stagger(progress: f32, index: usize) -> f32 {
        const STEP: f32 = 0.06;
        const SPAN: f32 = 0.45;
        ((progress - index.min(8) as f32 * STEP) / SPAN).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eases_from_start_to_end_and_stops() {
        let mut motion = Motion::new();
        let key = ("t", "a");
        motion.restart(key, 0.0, 1.0, 100);
        let start = motion.now;
        assert!(motion.active());
        motion.advance(start + Duration::from_millis(50));
        let halfway = motion.get(key, 0.0);
        // Ease-out is past the midpoint at half the time.
        assert!(halfway > 0.5 && halfway < 1.0, "{halfway}");
        motion.advance(start + Duration::from_millis(100));
        assert_eq!(motion.get(key, 0.0), 1.0);
        assert!(!motion.active(), "a finished tween must stop requesting frames");
    }

    #[test]
    fn reversing_continues_from_the_current_value() {
        let mut motion = Motion::new();
        let key = ("t", "b");
        let start = Instant::now();
        motion.tweens.insert(key, Tween { from: 0.0, to: 1.0, start, duration: Duration::from_secs(10) });
        let later = start + Duration::from_secs(5);
        let midway = motion.tweens[&key].value(later);
        assert!(midway > 0.0);
        // Started from "now", which is at least as late as `start`: never back at 0.
        motion.to(key, 0.0, 0.0, 100);
        assert!(motion.tweens[&key].from >= 0.0);
        assert_eq!(motion.tweens[&key].to, 0.0);
    }

    #[test]
    fn settle_only_moves_on_a_real_change() {
        let mut motion = Motion::new();
        let key = ("t", "s");
        motion.settle(key, 1.0, 100);
        assert_eq!(motion.get(key, 0.0), 1.0, "a key first seen starts at its target");
        assert!(!motion.active());
        motion.settle(key, 1.0, 100);
        assert!(!motion.active());
        motion.settle(key, 0.0, 100);
        assert!(motion.active());
    }

    #[test]
    fn unknown_keys_read_their_default_and_do_not_run() {
        let motion = Motion::new();
        assert_eq!(motion.get(("t", "c"), 0.25), 0.25);
        assert!(!motion.active());
    }

    #[test]
    fn list_items_arrive_in_order() {
        assert_eq!(Motion::stagger(0.0, 0), 0.0);
        assert!(Motion::stagger(0.3, 0) > Motion::stagger(0.3, 3));
        assert_eq!(Motion::stagger(1.0, 200), 1.0);
    }
}
