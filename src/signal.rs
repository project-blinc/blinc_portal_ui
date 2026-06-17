//! Per-portal signal subscription map.
//!
//! Every portal frame runs inside [`blinc_core::reactive::with_read_tracking`]
//! so we know exactly which signals the closure read. [`PortalSubscriptions`]
//! diffs that read-set against the previous frame's and updates a
//! process-global registry so future `Signal::set` calls flip the
//! reading portals' dirty flags. The next host frame observes the flag
//! and re-runs the portal closure.
//!
//! The registry is reverse-indexed both ways (portal → signals,
//! signal → portals) so dispatch is O(1) on either side. The notifier
//! collects affected dirty-flag handles under the registry lock,
//! releases the lock, then flips the atomics — the lock is never
//! re-entered from the notifier path, so portal code calling
//! `Signal::set` mid-frame can't deadlock on the registry.

use crate::core::PortalId;
use blinc_core::reactive::{set_portal_notifier, SignalId};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// Process-global subscription map shared across all portals.
///
/// Two indices in one struct so both directions are O(1):
/// - `by_portal[portal_id] = set of signals it currently reads`
/// - `by_signal[signal_id] = set of portals reading it`
///
/// `dirty_flags[portal_id]` is the atomic the portal's render loop
/// reads each frame (and clears after re-rendering).
#[derive(Default)]
struct GlobalRegistry {
    by_portal: HashMap<PortalId, HashSet<SignalId>>,
    by_signal: HashMap<SignalId, HashSet<PortalId>>,
    dirty_flags: HashMap<PortalId, Arc<AtomicBool>>,
}

static REGISTRY: OnceLock<Mutex<GlobalRegistry>> = OnceLock::new();

fn registry() -> &'static Mutex<GlobalRegistry> {
    REGISTRY.get_or_init(|| {
        // First touch installs the global reactive notifier so every
        // future `Signal::set` reaches us. Subsequent calls to
        // `set_portal_notifier` are silently dropped per the OnceLock
        // semantics in blinc_core::reactive — this is fine, we want
        // exactly one installer.
        set_portal_notifier(|sig_id| {
            on_signal_changed(sig_id);
        });
        Mutex::new(GlobalRegistry::default())
    })
}

/// Called from inside the reactive notifier on every `Signal::set`.
/// Flips the dirty flag of every portal that read this signal during
/// its last frame, plus requests a host redraw so the next composite
/// runs.
fn on_signal_changed(sig_id: SignalId) {
    let portals: Vec<Arc<AtomicBool>> = {
        let reg = registry().lock().expect("portal subs poisoned");
        let Some(set) = reg.by_signal.get(&sig_id) else {
            return;
        };
        set.iter()
            .filter_map(|pid| reg.dirty_flags.get(pid).cloned())
            .collect()
    };
    if portals.is_empty() {
        return;
    }
    for flag in &portals {
        flag.store(true, Ordering::Release);
    }
    blinc_layout::stateful::request_redraw();
}

/// Public handle the portal owns. Created via [`Self::new`] when the
/// portal is first constructed, dropped automatically when the portal
/// is — `Drop` cleans up the subscription map entries so a removed
/// portal stops being notified.
pub struct PortalSubscriptions {
    portal_id: PortalId,
    /// Atomic flag — set by [`on_signal_changed`] when any subscribed
    /// signal fires. The portal reads it each frame; if true, it
    /// repaints regardless of whether the canvas closure would
    /// otherwise be skipped.
    dirty: Arc<AtomicBool>,
}

impl PortalSubscriptions {
    /// Register a new portal in the subscription map and return its
    /// shared dirty flag. Idempotent per `portal_id`: re-registering
    /// resets the read set but keeps the dirty flag arc so existing
    /// references remain valid.
    pub fn new(portal_id: PortalId) -> Self {
        let dirty = {
            let mut reg = registry().lock().expect("portal subs poisoned");
            reg.by_portal.entry(portal_id).or_default().clear();
            reg.dirty_flags
                .entry(portal_id)
                .or_insert_with(|| Arc::new(AtomicBool::new(false)))
                .clone()
        };
        Self { portal_id, dirty }
    }

    /// True if any tracked signal fired since the last [`Self::clear_dirty`].
    pub fn is_dirty(&self) -> bool {
        self.dirty.load(Ordering::Acquire)
    }

    /// Atomically clear the dirty flag — call at the START of a
    /// frame's render so any signal change DURING render still flags
    /// us dirty for the NEXT frame.
    pub fn clear_dirty(&self) {
        self.dirty.store(false, Ordering::Release);
    }

    /// Shared clone of the dirty flag — useful when something other
    /// than the registry wants to flip it (animation tick, manual
    /// invalidation).
    pub fn dirty_flag(&self) -> Arc<AtomicBool> {
        self.dirty.clone()
    }

    /// Replace this portal's tracked signal set with `new_reads`,
    /// updating both indices in the registry. Subscriptions for
    /// signals no longer in the set are dropped; new ones are added.
    /// Cheap: only touches the diff.
    pub fn update_from_read_set(&self, new_reads: Vec<SignalId>) {
        let new: HashSet<SignalId> = new_reads.into_iter().collect();
        let mut reg = registry().lock().expect("portal subs poisoned");
        let old = reg.by_portal.entry(self.portal_id).or_default().clone();

        // Drop subscriptions for signals no longer read this frame.
        for sig in old.difference(&new) {
            if let Some(portals) = reg.by_signal.get_mut(sig) {
                portals.remove(&self.portal_id);
                if portals.is_empty() {
                    reg.by_signal.remove(sig);
                }
            }
        }
        // Add subscriptions for newly-read signals.
        for sig in new.difference(&old) {
            reg.by_signal
                .entry(*sig)
                .or_default()
                .insert(self.portal_id);
        }
        reg.by_portal.insert(self.portal_id, new);
    }
}

impl Drop for PortalSubscriptions {
    fn drop(&mut self) {
        // Pull every reverse-map entry for this portal out, so a
        // future signal change doesn't try to flip a dangling arc.
        let mut reg = registry().lock().expect("portal subs poisoned");
        if let Some(sigs) = reg.by_portal.remove(&self.portal_id) {
            for sig in sigs {
                if let Some(portals) = reg.by_signal.get_mut(&sig) {
                    portals.remove(&self.portal_id);
                    if portals.is_empty() {
                        reg.by_signal.remove(&sig);
                    }
                }
            }
        }
        reg.dirty_flags.remove(&self.portal_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blinc_core::reactive::signal;

    /// Drop semantics — Drop unsubscribes everywhere.
    #[test]
    fn drop_clears_registrations() {
        let sig = signal::<i32>(0);
        let sid = sig.id();

        let pid = PortalId(0x1111);
        let subs = PortalSubscriptions::new(pid);
        subs.update_from_read_set(vec![sid]);

        {
            let reg = registry().lock().unwrap();
            assert!(reg.by_signal.get(&sid).unwrap().contains(&pid));
            assert!(reg.by_portal.get(&pid).unwrap().contains(&sid));
        }

        drop(subs);

        let reg = registry().lock().unwrap();
        assert!(!reg.by_signal.contains_key(&sid) || !reg.by_signal[&sid].contains(&pid));
        assert!(!reg.by_portal.contains_key(&pid));
    }

    /// `Signal::set` fires the portal notifier and the right dirty
    /// flag flips — but ONLY for portals that read it.
    #[test]
    fn signal_change_flips_subscriber_dirty_only() {
        let sig_a = signal::<i32>(0);
        let sig_b = signal::<i32>(0);

        let p1 = PortalSubscriptions::new(PortalId(0xa));
        let p2 = PortalSubscriptions::new(PortalId(0xb));

        // p1 reads A; p2 reads B.
        p1.update_from_read_set(vec![sig_a.id()]);
        p2.update_from_read_set(vec![sig_b.id()]);

        p1.clear_dirty();
        p2.clear_dirty();

        sig_a.set(42);
        assert!(p1.is_dirty(), "p1 reads A → flipped");
        assert!(!p2.is_dirty(), "p2 doesn't read A → unchanged");
    }
}
