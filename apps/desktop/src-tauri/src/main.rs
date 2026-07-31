// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let mut arguments = std::env::args_os().skip(1);
    if arguments.next().as_deref() == Some(std::ffi::OsStr::new("--self-test-bundled-plugins")) {
        let Some(output) = arguments.next() else {
            std::process::exit(2);
        };
        if arguments.next().is_some() {
            std::process::exit(2);
        }
        let result =
            token_station_desktop_lib::run_installed_self_test(std::path::Path::new(&output));
        std::process::exit(if result.is_ok() { 0 } else { 1 });
    }
    token_station_desktop_lib::run()
}
