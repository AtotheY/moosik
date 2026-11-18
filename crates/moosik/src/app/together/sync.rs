//! Correct clock drift without repeatedly flushing the guest's decoder and network cache.
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct Sync {
    last: Option<(Instant, f64, bool)>,
    corrected: Option<Instant>,
    error: Option<f64>,
    far_samples: u8,
    pending_jump: bool,
    aligned: bool,
}

#[derive(Debug)]
pub(super) struct Correction {
    pub speed: f64,
    pub seek: Option<f64>,
}

impl Sync {
    pub fn stale(&self, now: Instant) -> bool {
        self.last
            .is_some_and(|(at, _, _)| now.duration_since(at) > Duration::from_secs(3))
    }

    pub fn update(
        &mut self,
        now: Instant,
        target: f64,
        playing: bool,
        local: &crate::listening::Sample,
    ) -> Correction {
        let previous = self.last.replace((now, target, playing));
        let mut result = Correction {
            speed: 1.0,
            seek: None,
        };
        self.pending_jump |= previous.is_some_and(|(at, pos, was_playing)| {
            let expected = pos
                + if was_playing {
                    now.duration_since(at).as_secs_f64()
                } else {
                    0.0
                };
            (target - expected).abs() > 1.5
        });
        if local.buffering || local.seeking || local.idle {
            self.error = None;
            self.far_samples = 0;
            self.corrected = Some(now);
            return result;
        }
        let drift = target - local.position;
        // Preserve a host seek observed while the guest was buffering or in cooldown.
        // It can be forgotten only after applying it, or naturally reaching its new clock.
        if drift.abs() <= 0.5 {
            self.pending_jump = false;
        }
        let settled = self
            .corrected
            .is_none_or(|at| now.duration_since(at) >= Duration::from_secs(3));
        self.far_samples = if drift.abs() > 2.0 {
            self.far_samples.saturating_add(1)
        } else {
            0
        };
        let first_alignment = settled && !self.aligned;
        if settled {
            self.aligned = true;
        }
        if settled
            && ((first_alignment && drift.abs() > 0.5)
                || (!playing && drift.abs() > 0.15)
                || self.pending_jump
                || (drift.abs() > 2.0 && (previous.is_none() || self.far_samples >= 2)))
        {
            result.seek = Some(target);
            self.corrected = Some(now);
            self.error = None;
            self.far_samples = 0;
            self.pending_jump = false;
            return result;
        }
        if playing && !local.paused && settled {
            let filtered = self.error.map_or(drift, |old| old * 0.75 + drift * 0.25);
            self.error = Some(filtered);
            if filtered.abs() > 0.15 {
                result.speed = 1.0 + (filtered * 0.03).clamp(-0.02, 0.02);
            }
        } else {
            self.error = None;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ten_minutes_of_clock_drift_and_network_jitter_never_seek() {
        let start = Instant::now();
        let mut sync = Sync::default();
        let mut local = crate::listening::Sample {
            position: 0.35,
            ..Default::default()
        };
        let mut speed = 1.0;
        for second in 1..=600 {
            local.position += speed * 0.998;
            let jitter = [0.0, 0.20, -0.12, 0.45, -0.08][second % 5];
            let correction = sync.update(
                start + Duration::from_secs(second as u64),
                second as f64 + jitter,
                true,
                &local,
            );
            assert!(
                correction.seek.is_none(),
                "unexpected seek at {second}: {correction:?}"
            );
            speed = correction.speed;
            assert!((0.98..=1.02).contains(&speed));
        }
        assert!((local.position - 600.0).abs() < 0.4);
    }

    #[test]
    fn a_late_join_aligns_once_then_corrects_small_drift_without_seeking() {
        let start = Instant::now();
        let mut sync = Sync::default();
        let mut local = crate::listening::Sample {
            position: 18.7,
            ..Default::default()
        };
        assert_eq!(sync.update(start, 20.0, true, &local).seek, Some(20.0));
        local.position = 23.7;
        let ongoing = sync.update(start + Duration::from_secs(5), 25.0, true, &local);
        assert!(ongoing.seek.is_none());
        assert_eq!(ongoing.speed, 1.02);
    }

    #[test]
    fn host_seek_during_buffering_is_applied_after_the_cooldown() {
        let start = Instant::now();
        let mut sync = Sync::default();
        let mut local = crate::listening::Sample {
            position: 10.0,
            ..Default::default()
        };
        sync.update(start, 10.0, true, &local);
        local.buffering = true;
        sync.update(start + Duration::from_secs(1), 11.0, true, &local);
        // A 1.8s host seek, smaller than the fallback threshold, arrives during buffering.
        assert!(
            sync.update(start + Duration::from_secs(2), 13.8, true, &local)
                .seek
                .is_none()
        );
        local.buffering = false;
        for second in 3..5 {
            local.position = 10.0 + second as f64;
            assert!(
                sync.update(
                    start + Duration::from_secs(second),
                    local.position + 1.8,
                    true,
                    &local
                )
                .seek
                .is_none()
            );
        }
        local.position = 15.0;
        assert_eq!(
            sync.update(start + Duration::from_secs(5), 16.8, true, &local)
                .seek,
            Some(16.8)
        );
    }

    #[test]
    fn host_seek_and_paused_seek_work_but_buffering_cannot_cause_a_seek_loop() {
        let start = Instant::now();
        let mut sync = Sync::default();
        let mut local = crate::listening::Sample {
            position: 10.0,
            ..Default::default()
        };
        assert!(sync.update(start, 10.0, true, &local).seek.is_none());
        assert_eq!(
            sync.update(start + Duration::from_secs(1), 50.0, true, &local)
                .seek,
            Some(50.0)
        );
        local.seeking = true;
        assert!(
            sync.update(start + Duration::from_secs(2), 51.0, true, &local)
                .seek
                .is_none()
        );
        local.seeking = false;
        local.position = 52.0;
        assert!(
            sync.update(start + Duration::from_secs(3), 52.0, true, &local)
                .seek
                .is_none()
        );
        local.paused = true;
        assert_eq!(
            sync.update(start + Duration::from_secs(6), 70.0, false, &local)
                .seek,
            Some(70.0)
        );
        local.position = 70.0;
        assert!(
            sync.update(start + Duration::from_secs(10), 70.0, false, &local)
                .seek
                .is_none()
        );
        assert!(sync.stale(start + Duration::from_secs(14)));
    }
}
