use aujar_core::Session;

pub fn detect_session() -> Session {
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
