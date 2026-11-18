use factory_core::context::factory::application::interfaces::startup_registration::StartupRegistration;
use tauri_plugin_autostart::ManagerExt;

/// Native adapter; admission and compensation belong to the application service.
pub(crate) struct AutolaunchRegistration<'a>(pub &'a tauri::AppHandle);

impl StartupRegistration for AutolaunchRegistration<'_> {
    fn is_enabled(&mut self) -> factory_core::Result<bool> {
        Ok(self.0.autolaunch().is_enabled()?)
    }

    fn set_enabled(&mut self, enabled: bool) -> factory_core::Result<()> {
        if enabled {
            self.0.autolaunch().enable()?;
        } else {
            self.0.autolaunch().disable()?;
        }
        Ok(())
    }
}
