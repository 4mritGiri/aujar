use aujar_core::{DesktopFeature, FeatureSupport, Platform, Session, SupportLevel};

pub const fn current_platform() -> Platform {
    Platform::current()
}

pub fn detect_session() -> Session {
    match current_platform() {
        Platform::Windows => return Session::Windows,
        Platform::MacOs => return Session::MacOs,
        Platform::Other => return Session::Unknown,
        Platform::Linux => {}
    }

    match std::env::var("XDG_SESSION_TYPE")
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "wayland" => Session::Wayland,
        "x11" => Session::X11,
        _ if std::env::var_os("WAYLAND_DISPLAY").is_some() => Session::Wayland,
        _ if std::env::var_os("DISPLAY").is_some() => Session::X11,
        _ => Session::Unknown,
    }
}

/// Report the capabilities implemented by the selected platform backend.
pub fn feature_support(platform: Platform) -> Vec<FeatureSupport> {
    use DesktopFeature::*;
    use SupportLevel::*;

    let portable = [PowerRename, ImageResizer];
    let mut report = DesktopFeature::ALL
        .into_iter()
        .map(|feature| FeatureSupport {
            feature,
            level: Planned,
        })
        .collect::<Vec<_>>();

    for entry in &mut report {
        if portable.contains(&entry.feature) {
            entry.level = Available;
        }
    }

    let set = |feature: DesktopFeature, level: SupportLevel, report: &mut Vec<FeatureSupport>| {
        if let Some(entry) = report.iter_mut().find(|entry| entry.feature == feature) {
            entry.level = level;
        }
    };

    match platform {
        Platform::Linux => {
            set(Launcher, Partial, &mut report);
            set(ColorPicker, Partial, &mut report);
            set(ClipboardHistory, Partial, &mut report);
            set(MouseWithoutBorders, Partial, &mut report);
            set(WindowManagement, Partial, &mut report);
            set(FancyZones, Partial, &mut report);
        }
        Platform::MacOs | Platform::Windows => {
            set(MouseWithoutBorders, Partial, &mut report);
        }
        Platform::Other => {}
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_tools_are_available_everywhere() {
        for platform in [
            Platform::Linux,
            Platform::MacOs,
            Platform::Windows,
            Platform::Other,
        ] {
            let report = feature_support(platform);
            assert!(
                report
                    .iter()
                    .any(|entry| entry.feature == DesktopFeature::PowerRename
                        && entry.level == SupportLevel::Available)
            );
            assert!(
                report
                    .iter()
                    .any(|entry| entry.feature == DesktopFeature::ImageResizer
                        && entry.level == SupportLevel::Available)
            );
        }
    }
}
