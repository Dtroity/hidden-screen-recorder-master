#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> eframe::Result<()> {
    let (config, warning) = match hidden_screen_recorder::config::AppConfig::load() {
        Ok(loaded) => loaded,
        Err(error) => (
            hidden_screen_recorder::config::AppConfig::default(),
            Some(error),
        ),
    };
    hidden_screen_recorder::debug_log::set_enabled(config.logging_enabled);
    hidden_screen_recorder::debug_log::reset();
    hidden_screen_recorder::debug_log::write("admin console started");
    let viewport = eframe::egui::ViewportBuilder::default()
        .with_inner_size(hidden_screen_recorder::WINDOW_SIZE)
        .with_min_inner_size(hidden_screen_recorder::MIN_WINDOW_SIZE)
        .with_title(hidden_screen_recorder::WINDOW_TITLE)
        .with_icon(hidden_screen_recorder::window::app_icon(false))
        .with_resizable(true);

    eframe::run_native(
        hidden_screen_recorder::WINDOW_TITLE,
        eframe::NativeOptions {
            viewport,
            ..Default::default()
        },
        Box::new(move |cc| {
            Ok(Box::new(
                hidden_screen_recorder::HiddenScreenRecorderApp::new(cc, config, warning),
            ))
        }),
    )
}
