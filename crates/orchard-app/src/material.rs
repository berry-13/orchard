//! Native window material behind the webview: Liquid Glass on macOS 26 and
//! later (vibrancy before that), Mica on Windows 11. The window is created
//! transparent on those platforms (see `tauri.macos.conf.json` and
//! `tauri.windows.conf.json`); the UI only lets the material show through
//! once it is told one was applied, so a failure leaves an opaque window.

use tauri::WebviewWindow;

/// The material applied to the main window, if any.
#[derive(Debug, Default)]
pub struct WindowMaterial(pub Option<&'static str>);

#[cfg(target_os = "macos")]
pub fn apply(window: &WebviewWindow) -> Option<&'static str> {
    use window_vibrancy::{
        LiquidGlassOptions, NSGlassEffectViewStyle, NSVisualEffectMaterial, NSVisualEffectState,
        apply_liquid_glass, apply_vibrancy,
    };

    if apply_liquid_glass(
        window,
        LiquidGlassOptions::new(NSGlassEffectViewStyle::Regular),
    )
    .is_ok()
    {
        return Some("liquidGlass");
    }
    apply_vibrancy(
        window,
        NSVisualEffectMaterial::Sidebar,
        Some(NSVisualEffectState::FollowsWindowActiveState),
        None,
    )
    .ok()
    .map(|()| "vibrancy")
}

#[cfg(target_os = "windows")]
pub fn apply(window: &WebviewWindow) -> Option<&'static str> {
    // Fails before Windows 11, where the window stays opaque.
    window_vibrancy::apply_mica(window, None)
        .ok()
        .map(|()| "mica")
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn apply(_window: &WebviewWindow) -> Option<&'static str> {
    None
}
