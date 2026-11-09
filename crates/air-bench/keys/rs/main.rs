// The launcher: the app is the library, so `ax dev` can hot-patch all of it.
// A release build on Windows is a window app: no console opens with it.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
use aio_shell::Start;

fn main() -> std::io::Result<()> {
    let cfg = aio::config!();
    let Start::Run(dirs) = aio_shell::start(&cfg, aio::app_version!())? else { return Ok(()) };
    aio_shell::run(app::App, aio_shell::Window { width: 1000, height: 700, devtools: true, ..aio_shell::Window::of(&cfg) }, &dirs)
}
