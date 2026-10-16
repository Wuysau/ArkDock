//! A profile is committed only after the new runtime produces its first frame.
//! The engine coordinates cancellation, restart and recovery; this state never
//! starts a process or changes project/session/source identity.
use super::configuration::{default_profile, profile_arguments};
use crate::ipc::model::PreviewDeviceProfile;

#[derive(Clone, Debug)]
pub struct ProfileSelection {
    selected: PreviewDeviceProfile,
    previous: Option<PreviewDeviceProfile>,
}

impl Default for ProfileSelection {
    fn default() -> Self {
        Self {
            selected: default_profile(),
            previous: None,
        }
    }
}

impl ProfileSelection {
    pub fn current(&self) -> &PreviewDeviceProfile {
        &self.selected
    }
    pub fn pending(&self) -> bool {
        self.previous.is_some()
    }

    /// Reconcile changed deviceTypes before launch, including the same project.
    /// An explicit in-flight switch is allowed to fail and roll back instead of
    /// silently replacing the user's choice.
    pub fn reconcile(
        &mut self,
        catalog: &[PreviewDeviceProfile],
        new_project: bool,
    ) -> Result<(), String> {
        use crate::ipc::model::NativeControlAvailability::Available;
        if !new_project {
            if catalog
                .iter()
                .any(|profile| profile.id == self.selected.id && profile.status == Available)
            {
                return Ok(());
            }
            if self.pending() {
                return Err("PROFILE_PENDING_NO_LONGER_SUPPORTED".into());
            }
        }
        let eligible = catalog
            .iter()
            .find(|profile| profile.status == Available)
            .ok_or_else(|| {
                catalog
                    .first()
                    .map(|profile| profile.reason.clone())
                    .unwrap_or_else(|| "PROFILE_PROJECT_REQUIRED".into())
            })?;
        profile_arguments(eligible)?;
        self.selected = eligible.clone();
        self.previous = None;
        Ok(())
    }

    pub fn select(
        &mut self,
        profile: PreviewDeviceProfile,
    ) -> Result<PreviewDeviceProfile, String> {
        profile_arguments(&profile)?;
        if self.pending() {
            return Err("PROFILE_SWITCH_PENDING".into());
        }
        if self.selected.id == profile.id {
            return Err("PROFILE_ALREADY_SELECTED".into());
        }
        let previous = self.selected.clone();
        self.previous = Some(previous.clone());
        self.selected = profile;
        Ok(previous)
    }

    pub fn confirm(&mut self) {
        self.previous = None;
    }

    /// Retain the previous confirmed selection after a failed start. The caller
    /// must explicitly schedule its recovery runtime; changing this value alone
    /// is not a claim that recovery succeeded.
    pub fn rollback(&mut self) -> bool {
        if let Some(previous) = self.previous.take() {
            self.selected = previous;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview::configuration::{profiles, DEFAULT_PROFILE};

    #[test]
    fn changed_device_types_reselect_without_overriding_pending_switch() {
        use crate::ipc::model::NativeControlAvailability;
        let mut state = ProfileSelection::default();
        let mut catalog = profiles(true);
        catalog[0].status = NativeControlAvailability::Blocked;
        catalog[1].status = NativeControlAvailability::Blocked;
        state.reconcile(&catalog, false).unwrap();
        assert_eq!(state.current().device_type, "wearable");
        state.select(profiles(true).remove(1)).unwrap();
        assert!(state.reconcile(&catalog, false).is_err());
        assert_eq!(state.current().device_type, "tablet");
        assert!(state.rollback());
        assert_eq!(state.current().device_type, "wearable");
        state.reconcile(&profiles(true), true).unwrap();
        assert_eq!(state.current().device_type, "phone");
    }

    #[test]
    fn failed_start_restores_previous_profile_and_can_retry() {
        let mut state = ProfileSelection::default();
        let tablet = profiles(true).remove(1);
        assert_eq!(state.select(tablet.clone()).unwrap().id, DEFAULT_PROFILE);
        assert!(state.pending());
        assert_eq!(state.current().id, tablet.id);
        assert_eq!(
            state.select(profiles(true).remove(2)).unwrap_err(),
            "PROFILE_SWITCH_PENDING"
        );
        assert!(state.rollback());
        assert_eq!(state.current().id, DEFAULT_PROFILE);
        assert!(!state.rollback());
        assert!(state.select(tablet).is_ok());
    }

    #[test]
    fn success_commits_profile_without_later_spurious_rollback() {
        let mut state = ProfileSelection::default();
        let watch = profiles(true).remove(2);
        state.select(watch.clone()).unwrap();
        state.confirm();
        assert!(!state.pending());
        assert!(!state.rollback());
        assert_eq!(state.current().id, watch.id);
        assert_eq!(state.select(watch).unwrap_err(), "PROFILE_ALREADY_SELECTED");
    }

    #[test]
    fn new_runtime_inherits_selected_geometry_but_waits_for_its_own_frame() {
        for profile in profiles(true) {
            let runtime = crate::preview::control::RuntimeControlState::from_profile(&profile);
            assert_eq!(
                (runtime.width, runtime.height, runtime.dpi),
                (profile.width, profile.height, profile.dpi)
            );
            assert!(!runtime.input_ready);
            assert!(runtime.current_route.is_none());
        }
    }
}
