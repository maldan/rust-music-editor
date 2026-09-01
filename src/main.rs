#![windows_subsystem = "windows"]

mod app;
mod compile;
mod framework;
mod graph;
mod monitor;
mod ui;

use app::App;
use framework::Host;

fn main() {
    let app = App::start().expect("audio device");
    Host::run(app);
}
