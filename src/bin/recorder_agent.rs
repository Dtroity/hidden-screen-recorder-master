#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Err(error) = hidden_screen_recorder::agent::run() {
        eprintln!("recorder-agent error: {error}");
        std::process::exit(1);
    }
}
