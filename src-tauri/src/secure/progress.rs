//! Bounded one-second throughput window. Time and byte counts are supplied by IO.
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

pub(super) struct Meter {
    start: Instant,
    samples: VecDeque<(Instant, u64)>,
}
impl Meter {
    pub fn new(now: Instant) -> Self {
        Self {
            start: now,
            samples: VecDeque::from([(now, 0)]),
        }
    }
    pub fn sample(
        &mut self,
        now: Instant,
        transferred: u64,
        total: u64,
    ) -> (Option<f64>, Option<u64>) {
        self.samples.push_back((now, transferred));
        while self.samples.len() > 5 {
            self.samples.pop_front();
        }
        while self.samples.len() > 2
            && now.duration_since(self.samples[1].0) >= Duration::from_secs(1)
        {
            self.samples.pop_front();
        }
        let (then, bytes) = self.samples[0];
        let elapsed = now.duration_since(then).as_secs_f64();
        if elapsed < 0.2 {
            return (None, None);
        }
        let rate = transferred.saturating_sub(bytes) as f64 / elapsed;
        let estimate = (total.saturating_sub(transferred) as f64 / rate).ceil();
        let eta = (now.duration_since(self.start) >= Duration::from_secs(1)
            && transferred < total
            && rate > 0.0
            && estimate.is_finite()
            && estimate <= 86_400.0)
            .then_some(estimate as u64);
        (Some(rate), eta)
    }
}
pub(super) fn begin(s: &super::Shared, connection: &str) {
    let state = &mut *super::lock(&s.state);
    if let Some(entry) = state.entries.iter_mut().find(|e| e.view.id == connection) {
        entry.meter = Some(Meter::new(Instant::now()));
        entry.progress_wake.notify_one();
    }
}
pub(super) fn record(s: &super::Shared, connection: &str, count: usize) {
    let mut state = super::lock(&s.state);
    if let Some(entry) = state.entries.iter_mut().find(|e| e.view.id == connection)
        && let Some(transfer) = &mut entry.view.transfer
    {
        transfer.transferred = transfer
            .transferred
            .saturating_add(count as u64)
            .min(transfer.size);
    }
}
fn sample(s: &super::Shared, connection: &str) -> Option<super::Snapshot> {
    let mut state = super::lock(&s.state);
    let entry = state.entries.iter_mut().find(|e| e.view.id == connection)?;
    let transfer = entry.view.transfer.as_mut()?;
    if !matches!(transfer.status.as_str(), "sending" | "receiving") {
        return None;
    }
    let meter = entry.meter.as_mut()?;
    let (speed, eta) = meter.sample(Instant::now(), transfer.transferred, transfer.size);
    transfer.bytes_per_second = speed;
    transfer.eta_seconds = eta;
    state.snapshot.revision = state.snapshot.revision.saturating_add(1);
    state.snapshot.connections = state.entries.iter().map(|e| e.view.clone()).collect();
    Some(state.snapshot.clone())
}
pub(super) async fn run(
    s: std::sync::Arc<super::Shared>,
    connection: String,
    wake: std::sync::Arc<tokio::sync::Notify>,
) {
    loop {
        wake.notified().await;
        loop {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let Some(snapshot) = sample(&s, &connection) else {
                break;
            };
            (s.notify)(snapshot);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn measures_actual_bytes_and_delays_eta_until_reliable() {
        let now = Instant::now();
        let mut meter = Meter::new(now);
        assert_eq!(meter.sample(now, 0, 1000), (None, None));
        let (rate, eta) = meter.sample(now + Duration::from_millis(500), 100, 1000);
        assert_eq!(rate, Some(200.0));
        assert_eq!(eta, None);
        assert_eq!(
            meter.sample(now + Duration::from_secs(1), 200, 1000),
            (Some(200.0), Some(4))
        );
        assert_eq!(
            meter.sample(now + Duration::from_secs(3), 200, 1000),
            (Some(0.0), None)
        );
    }
    #[test]
    fn empty_and_completed_files_have_no_eta_and_window_is_bounded() {
        let now = Instant::now();
        let mut meter = Meter::new(now);
        for i in 1..100 {
            let (_, eta) = meter.sample(now + Duration::from_millis(i * 250), 0, 0);
            assert_eq!(eta, None);
            assert!(meter.samples.len() <= 5);
        }
    }
}
