//! Process-local startup gate; only the platform permission callback resolves it.
use std::sync::atomic::{AtomicU8, Ordering};
#[derive(Default)]
pub(crate) struct NetworkPermission(AtomicU8);
impl NetworkPermission {
    pub(crate) const fn new() -> Self {
        Self(AtomicU8::new(0))
    }
    pub(crate) fn allowed(&self) -> bool {
        self.0.load(Ordering::Acquire) == 1
    }
    pub(crate) fn denied(&self) -> bool {
        self.0.load(Ordering::Acquire) == 2
    }
    pub(crate) fn resolve(&self, granted: bool) {
        // One platform response per startup. A late/duplicate callback cannot
        // turn an already denied process into an implicitly authorized one.
        let _ = self.0.compare_exchange(
            0,
            if granted { 1 } else { 2 },
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }
}
#[cfg(target_os = "android")]
pub(crate) static NETWORK: NetworkPermission = NetworkPermission::new();
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pending_and_denied_never_allow_network_access() {
        let gate = NetworkPermission::new();
        assert!(!gate.allowed() && !gate.denied());
        gate.resolve(false);
        assert!(!gate.allowed() && gate.denied());
        gate.resolve(true);
        assert!(
            !gate.allowed(),
            "Denial needs a fresh application permission check"
        );
    }
    #[test]
    fn grant_requires_the_platform_response_and_is_process_local() {
        let gate = NetworkPermission::new();
        gate.resolve(true);
        assert!(gate.allowed() && !gate.denied());
        assert!(!NetworkPermission::new().allowed());
    }
}
