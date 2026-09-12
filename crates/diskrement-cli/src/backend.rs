//! Pure burner/device resolution (`BackendResolver.scala`).

use diskrement_shell::burn::BurnDevice;

use crate::args::BurnerKind;

/// Which backend a run should use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Backend {
    Simulated,
    Real(BurnDevice),
}

/// Precedence: flag > environment > default (the real burner on `/dev/sr0`).
///
/// Deliberately asymmetric, matching `BackendResolver.scala`: the flag is an
/// enumeration, so a bad value is an argument error, while an unrecognised
/// `DISKREMENT_BURNER` falls through to the default instead of failing.
pub fn resolve(
    burner_flag: Option<BurnerKind>,
    device_flag: Option<&str>,
    env: &dyn Fn(&str) -> Option<String>,
) -> Backend {
    let kind = burner_flag.or_else(|| {
        match env("DISKREMENT_BURNER")
            .map(|v| v.trim().to_lowercase())
            .as_deref()
        {
            Some("simulated") => Some(BurnerKind::Simulated),
            Some("real") => Some(BurnerKind::Real),
            _ => None,
        }
    });
    match kind.unwrap_or(BurnerKind::Real) {
        BurnerKind::Simulated => Backend::Simulated,
        BurnerKind::Real => {
            let device = device_flag
                .map(str::to_string)
                .or_else(|| env("DISKREMENT_DEVICE").filter(|v| !v.is_empty()))
                .unwrap_or_else(|| "/dev/sr0".to_string());
            Backend::Real(BurnDevice::RealDevice(device))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn the_default_is_the_real_burner_on_dev_sr0() {
        assert_eq!(
            resolve(None, None, &no_env),
            Backend::Real(BurnDevice::RealDevice("/dev/sr0".to_string()))
        );
    }

    #[test]
    fn the_flag_beats_the_environment() {
        let env = |k: &str| (k == "DISKREMENT_BURNER").then(|| "real".to_string());
        assert_eq!(
            resolve(Some(BurnerKind::Simulated), None, &env),
            Backend::Simulated
        );
    }

    #[test]
    fn the_environment_selects_the_simulated_burner() {
        let env = |k: &str| (k == "DISKREMENT_BURNER").then(|| "  SIMULATED \n".to_string());
        assert_eq!(resolve(None, None, &env), Backend::Simulated);
    }

    /// An unrecognised env value falls through to the default rather than
    /// failing — the flag is an enumeration, the environment is not.
    #[test]
    fn an_unknown_environment_value_falls_back_to_the_default() {
        let env = |k: &str| (k == "DISKREMENT_BURNER").then(|| "pretend".to_string());
        assert_eq!(
            resolve(None, None, &env),
            Backend::Real(BurnDevice::RealDevice("/dev/sr0".to_string()))
        );
    }

    #[test]
    fn the_device_comes_from_the_flag_then_the_environment_then_the_default() {
        let env = |k: &str| (k == "DISKREMENT_DEVICE").then(|| "/dev/sr9".to_string());
        assert_eq!(
            resolve(None, Some("/dev/sr1"), &env),
            Backend::Real(BurnDevice::RealDevice("/dev/sr1".to_string()))
        );
        assert_eq!(
            resolve(None, None, &env),
            Backend::Real(BurnDevice::RealDevice("/dev/sr9".to_string()))
        );
    }

    /// An empty `DISKREMENT_DEVICE` is ignored (`filter(_.nonEmpty)`).
    #[test]
    fn an_empty_device_variable_is_ignored() {
        let env = |k: &str| (k == "DISKREMENT_DEVICE").then(String::new);
        assert_eq!(
            resolve(None, None, &env),
            Backend::Real(BurnDevice::RealDevice("/dev/sr0".to_string()))
        );
    }
}
