use crate::Result;

/// OS login registration. The application admits the operation before calling this port.
pub trait StartupRegistration {
    fn is_enabled(&mut self) -> Result<bool>;
    fn set_enabled(&mut self, enabled: bool) -> Result<()>;
}
