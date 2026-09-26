// alerts/machine.rs — the per-(rule, target) state machine, as pure functions
//
//   inactive ──true──▶ pending ──true for `for`──▶ firing ──false──▶ inactive
//       ▲                 │                                 (resolved)
//       └─────false───────┘ (cleared: flapping never fires)
//
// With `for` = 0 an inactive alert fires on the first true sample. Missing
// data (`None`) holds the current phase. Notifications are decided
// separately (`due_notification`): the first one when firing starts (or when
// a mute ends while still firing), then one every `repeat` until someone
// acknowledges it.

use crate::notifications::NoticeEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Inactive,
    Pending { since: i64 },
    Firing { since: i64, fired_at: i64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    Stay,
    BecamePending,
    Fired,
    Resolved,
    /// Pending → inactive before `for` elapsed.
    Cleared,
}

/// Advance one (rule, target) given the latest observation.
pub fn step(phase: Phase, holds: Option<bool>, now: i64, for_secs: i64) -> (Phase, Transition) {
    let Some(holds) = holds else {
        return (phase, Transition::Stay);
    };
    match (phase, holds) {
        (Phase::Inactive, false) => (phase, Transition::Stay),
        (Phase::Inactive, true) if for_secs <= 0 => (
            Phase::Firing {
                since: now,
                fired_at: now,
            },
            Transition::Fired,
        ),
        (Phase::Inactive, true) => (Phase::Pending { since: now }, Transition::BecamePending),
        (Phase::Pending { since }, true) if now - since >= for_secs => (
            Phase::Firing {
                since,
                fired_at: now,
            },
            Transition::Fired,
        ),
        (Phase::Pending { .. }, true) => (phase, Transition::Stay),
        (Phase::Pending { .. }, false) => (Phase::Inactive, Transition::Cleared),
        (Phase::Firing { .. }, true) => (phase, Transition::Stay),
        (Phase::Firing { .. }, false) => (Phase::Inactive, Transition::Resolved),
    }
}

/// Whether any of the mutes (rule, target, episode snooze) is still active.
pub fn muted(now: i64, until: &[Option<i64>]) -> bool {
    until.iter().flatten().any(|t| *t > now)
}

/// The notification a firing alert owes right now, if any.
pub fn due_notification(
    last_notified: Option<i64>,
    now: i64,
    repeat_secs: i64,
    acknowledged: bool,
    muted: bool,
) -> Option<NoticeEvent> {
    if muted {
        return None;
    }
    match last_notified {
        None => Some(NoticeEvent::Firing),
        Some(t) if !acknowledged && repeat_secs > 0 && now - t >= repeat_secs => {
            Some(NoticeEvent::Repeat)
        }
        Some(_) => None,
    }
}

/// Delay before retry number `attempt` (1-based) of a failed delivery.
pub fn retry_delay(attempt: i64) -> i64 {
    match attempt {
        ..=1 => 30,
        2 => 120,
        3 => 600,
        4 => 1800,
        _ => 7200,
    }
}

/// Attempts after which a delivery is given up (about 3 hours in total).
pub const MAX_ATTEMPTS: i64 = 6;

#[cfg(test)]
mod tests {
    use super::*;

    /// Drive a sequence of (seconds, observation) through `step`.
    fn run(for_secs: i64, samples: &[(i64, Option<bool>)]) -> Vec<(Phase, Transition)> {
        let mut phase = Phase::Inactive;
        samples
            .iter()
            .map(|(t, holds)| {
                let (next, tr) = step(phase, *holds, *t, for_secs);
                phase = next;
                (next, tr)
            })
            .collect()
    }

    #[test]
    fn pending_becomes_firing_only_after_for() {
        let out = run(
            300,
            &[
                (0, Some(true)),
                (150, Some(true)),
                (299, Some(true)),
                (300, Some(true)),
            ],
        );
        assert_eq!(
            out[0],
            (Phase::Pending { since: 0 }, Transition::BecamePending)
        );
        assert_eq!(out[1].1, Transition::Stay);
        assert_eq!(out[2].1, Transition::Stay);
        assert_eq!(
            out[3],
            (
                Phase::Firing {
                    since: 0,
                    fired_at: 300
                },
                Transition::Fired
            )
        );
    }

    #[test]
    fn zero_for_fires_on_the_first_true_sample() {
        let out = run(0, &[(10, Some(false)), (40, Some(true))]);
        assert_eq!(out[0], (Phase::Inactive, Transition::Stay));
        assert_eq!(out[1].1, Transition::Fired);
    }

    #[test]
    fn flapping_faster_than_for_never_fires() {
        let samples: Vec<(i64, Option<bool>)> =
            (0..20).map(|i| (i * 30, Some(i % 2 == 0))).collect();
        let out = run(120, &samples);
        assert!(out.iter().all(|(_, t)| *t != Transition::Fired), "{out:?}");
        assert_eq!(out[1].1, Transition::Cleared);
    }

    #[test]
    fn firing_resolves_on_the_first_false_sample_and_missing_data_holds() {
        let out = run(
            0,
            &[
                (0, Some(true)),
                (30, None),
                (60, Some(true)),
                (90, Some(false)),
                (120, None),
            ],
        );
        assert_eq!(out[1].1, Transition::Stay, "no data keeps it firing");
        assert!(matches!(out[2].0, Phase::Firing { .. }));
        assert_eq!(out[3], (Phase::Inactive, Transition::Resolved));
        assert_eq!(out[4], (Phase::Inactive, Transition::Stay));
    }

    #[test]
    fn missing_data_does_not_reset_pending() {
        let out = run(60, &[(0, Some(true)), (30, None), (60, Some(true))]);
        assert_eq!(out[2].1, Transition::Fired);
    }

    #[test]
    fn notifications_first_then_every_repeat_until_acknowledged() {
        let h4 = 4 * 3600;
        assert_eq!(
            due_notification(None, 0, h4, false, false),
            Some(NoticeEvent::Firing)
        );
        assert_eq!(due_notification(Some(0), h4 - 1, h4, false, false), None);
        assert_eq!(
            due_notification(Some(0), h4, h4, false, false),
            Some(NoticeEvent::Repeat)
        );
        assert_eq!(
            due_notification(Some(0), h4 * 3, h4, true, false),
            None,
            "acknowledged"
        );
        assert_eq!(
            due_notification(Some(0), h4 * 3, 0, false, false),
            None,
            "repeat off"
        );
        assert_eq!(due_notification(None, 0, h4, false, true), None, "snoozed");
        // A mute that ends while still firing: the first notice goes out then.
        assert_eq!(
            due_notification(None, 99, h4, true, false),
            Some(NoticeEvent::Firing)
        );
    }

    #[test]
    fn mutes_expire() {
        assert!(muted(10, &[None, Some(11)]));
        assert!(!muted(11, &[Some(11), None]));
        assert!(!muted(10, &[]));
    }

    #[test]
    fn retries_back_off_and_stop() {
        let total: i64 = (1..MAX_ATTEMPTS).map(retry_delay).sum();
        assert_eq!(retry_delay(1), 30);
        assert!(retry_delay(4) > retry_delay(3));
        assert!(total < 4 * 3600, "gives up within a few hours: {total}");
    }
}
