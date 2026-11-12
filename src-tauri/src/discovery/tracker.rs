use super::Device;
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

pub(super) const STALE_AFTER: Duration = Duration::from_secs(75);
const MAX_OBSERVATIONS: usize = 512;

struct Observation {
    device: Device,
    heartbeat: u64,
    last_seen: Instant,
    active: bool,
}

/// Keep DNS service sources separately: removing one alias must not remove another.
pub(super) struct Tracker {
    local_id: String,
    observations: BTreeMap<String, Observation>,
}

impl Tracker {
    pub fn new(local_id: String) -> Self {
        Self {
            local_id,
            observations: BTreeMap::new(),
        }
    }

    pub fn observe(&mut self, fullname: &str, device: Device, heartbeat: u64, now: Instant) {
        if device.id == self.local_id {
            return;
        }
        let key = fullname.to_ascii_lowercase();
        if !self.observations.contains_key(&key) && self.observations.len() >= MAX_OBSERVATIONS {
            self.expire(now);
            let oldest_inactive = self
                .observations
                .iter()
                .filter(|(_, observation)| !observation.active)
                .min_by_key(|(_, observation)| observation.last_seen)
                .map(|(key, _)| key.clone());
            if let Some(oldest) = oldest_inactive {
                self.observations.remove(&oldest);
            } else {
                return;
            }
        }
        if let Some(previous) = self.observations.get(&key) {
            // Cached/duplicate responses are not proof of life. Only a new heartbeat is.
            if previous.device.id == device.id && previous.heartbeat == heartbeat {
                return;
            }
        }
        self.observations.insert(
            key,
            Observation {
                device,
                heartbeat,
                last_seen: now,
                active: true,
            },
        );
    }

    pub fn remove(&mut self, fullname: &str) {
        if let Some(observation) = self.observations.get_mut(&fullname.to_ascii_lowercase()) {
            // Keep the marker so delayed cache responses cannot undo a goodbye.
            observation.active = false;
        }
    }

    pub fn expire(&mut self, now: Instant) {
        for observation in self.observations.values_mut() {
            if now.saturating_duration_since(observation.last_seen) >= STALE_AFTER {
                observation.active = false;
            }
        }
    }

    pub fn devices(&self) -> Vec<Device> {
        let mut unique: BTreeMap<&str, &Observation> = BTreeMap::new();
        for observation in self
            .observations
            .values()
            .filter(|observation| observation.active)
        {
            let current = unique.entry(&observation.device.id).or_insert(observation);
            if observation.last_seen > current.last_seen {
                *current = observation;
            }
        }
        let mut devices: Vec<Device> = unique.values().map(|v| v.device.clone()).collect();
        devices.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
        devices
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn device(id: &str) -> Device {
        Device {
            id: id.into(),
            name: format!("InvarDrop {id}"),
        }
    }
    fn tracker() -> Tracker {
        Tracker::new("self".into())
    }

    #[test]
    fn excludes_self_but_not_another_instance_on_the_same_host() {
        let mut t = tracker();
        let now = Instant::now();
        t.observe("self.service", device("self"), 0, now);
        t.observe("peer.service", device("peer"), 0, now);
        assert_eq!(t.devices(), vec![device("peer")]);
    }
    #[test]
    fn deduplicates_ids_and_dns_case_without_losing_aliases() {
        let mut t = tracker();
        let now = Instant::now();
        t.observe("Peer.SERVICE", device("peer"), 0, now);
        t.observe("peer.service", device("peer"), 1, now);
        t.observe("alias.service", device("peer"), 1, now);
        assert_eq!(t.observations.len(), 2);
        assert_eq!(t.devices(), vec![device("peer")]);
        t.remove("PEER.service");
        assert_eq!(t.devices(), vec![device("peer")]);
        t.remove("alias.service");
        assert!(t.devices().is_empty());
    }
    #[test]
    fn expires_exactly_at_deadline_and_ignores_duplicate_cache_responses() {
        let mut t = tracker();
        let now = Instant::now();
        t.observe("peer", device("peer"), 0, now);
        t.observe("peer", device("peer"), 0, now + Duration::from_secs(70));
        t.expire(now + STALE_AFTER - Duration::from_millis(1));
        assert_eq!(t.devices().len(), 1);
        t.expire(now + STALE_AFTER);
        assert!(t.devices().is_empty());
    }
    #[test]
    fn heartbeat_refreshes_expiration_without_duplicating_devices() {
        let mut t = tracker();
        let now = Instant::now();
        t.observe("peer", device("peer"), 0, now);
        t.observe("peer", device("peer"), 1, now + Duration::from_secs(20));
        t.expire(now + STALE_AFTER);
        assert_eq!(t.devices().len(), 1);
        t.expire(now + STALE_AFTER + Duration::from_secs(20));
        assert!(t.devices().is_empty());
    }
    #[test]
    fn renamed_device_updates_and_old_alias_expiration_preserves_live_alias() {
        let mut t = tracker();
        let now = Instant::now();
        t.observe("old", device("peer"), 0, now);
        let renamed = Device {
            id: "peer".into(),
            name: "New name".into(),
        };
        t.observe("new", renamed.clone(), 1, now + Duration::from_secs(20));
        t.expire(now + STALE_AFTER);
        assert_eq!(t.devices(), vec![renamed]);
    }
    #[test]
    fn bounds_untrusted_network_state_but_allows_existing_updates() {
        let mut t = tracker();
        let now = Instant::now();
        for i in 0..MAX_OBSERVATIONS + 10 {
            t.observe(&i.to_string(), device(&i.to_string()), 0, now);
        }
        assert_eq!(t.observations.len(), MAX_OBSERVATIONS);
        t.observe("0", device("0"), 1, now + Duration::from_secs(20));
        t.expire(now + STALE_AFTER);
        assert_eq!(t.devices(), vec![device("0")]);
    }
    #[test]
    fn cached_responses_cannot_revive_expired_or_removed_sources() {
        for goodbye in [false, true] {
            let mut t = tracker();
            let now = Instant::now();
            t.observe("peer", device("peer"), 0, now);
            if goodbye {
                t.remove("peer");
            } else {
                t.expire(now + STALE_AFTER);
            }
            t.observe("peer", device("peer"), 0, now + STALE_AFTER);
            assert!(t.devices().is_empty());
            t.observe("peer", device("peer"), 1, now + STALE_AFTER);
            assert_eq!(t.devices(), vec![device("peer")]);
        }
    }

    #[test]
    fn inactive_markers_are_bounded_and_evicted_to_admit_new_devices() {
        let mut t = tracker();
        let now = Instant::now();
        for i in 0..MAX_OBSERVATIONS {
            t.observe(&i.to_string(), device(&i.to_string()), 0, now);
        }
        t.expire(now + STALE_AFTER);
        t.observe("new", device("new"), 0, now + STALE_AFTER);
        assert_eq!(t.observations.len(), MAX_OBSERVATIONS);
        assert_eq!(t.devices(), vec![device("new")]);
    }
}
