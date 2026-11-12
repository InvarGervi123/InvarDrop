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
            return;
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
            },
        );
    }

    pub fn remove(&mut self, fullname: &str) {
        self.observations.remove(&fullname.to_ascii_lowercase());
    }

    pub fn expire(&mut self, now: Instant) {
        self.observations
            .retain(|_, v| now.saturating_duration_since(v.last_seen) < STALE_AFTER);
    }

    pub fn devices(&self) -> Vec<Device> {
        let mut unique: BTreeMap<&str, &Observation> = BTreeMap::new();
        for observation in self.observations.values() {
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
}
