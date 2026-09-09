#![windows_subsystem = "windows"]

mod app;
mod compile;
mod export;
mod fft;
mod framework;
mod graph;
mod monitor;
mod ui;
mod viz;

use app::App;
use framework::Host;

fn main() {
    let app = App::start().expect("audio device");
    Host::run(app);
}
