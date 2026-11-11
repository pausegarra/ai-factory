/// Keeps OS login registration and persisted user preference consistent.
pub fn change_with_persistence<T>(
    current: bool,
    desired: bool,
    mut set_startup: impl FnMut(bool) -> Result<(), String>,
    persist: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    if current == desired {
        return persist();
    }
    set_startup(desired)?;
    match persist() {
        Ok(value) => Ok(value),
        Err(error) => match set_startup(current) {
            Ok(()) => Err(error),
            Err(rollback) => Err(format!(
                "{error}. Could not restore login setting: {rollback}"
            )),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn failed_save_restores_previous_os_registration() {
        let changes = RefCell::new(Vec::new());
        let result: Result<(), String> = change_with_persistence(
            false,
            true,
            |enabled| {
                changes.borrow_mut().push(enabled);
                Ok(())
            },
            || Err("disk full".into()),
        );
        assert_eq!(result.unwrap_err(), "disk full");
        assert_eq!(*changes.borrow(), vec![true, false]);
    }

    #[test]
    fn registration_failure_does_not_save_preference() {
        let result: Result<(), String> = change_with_persistence(
            false,
            true,
            |_| Err("login permission denied".into()),
            || panic!("settings must not be saved after OS failure"),
        );
        assert_eq!(result.unwrap_err(), "login permission denied");
    }

    #[test]
    fn unchanged_registration_does_not_rewrite_os_entry() {
        let result = change_with_persistence(true, true, |_| panic!("unchanged"), || Ok(42));
        assert_eq!(result.unwrap(), 42);
    }

    #[test]
    fn rollback_failure_reports_both_errors() {
        let mut calls = 0;
        let result: Result<(), String> = change_with_persistence(
            false,
            true,
            |_| {
                calls += 1;
                if calls == 1 {
                    Ok(())
                } else {
                    Err("rollback denied".into())
                }
            },
            || Err("disk full".into()),
        );
        let error = result.unwrap_err();
        assert!(error.contains("disk full"));
        assert!(error.contains("rollback denied"));
    }
}
