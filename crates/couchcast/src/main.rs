//! Couchcast — fullscreen HDMI-capture viewer with controller input forwarding.

mod app;
mod worker;

use gtk4::glib;
use gtk4::prelude::*;

fn main() -> glib::ExitCode {
    init_tracing();
    let application = app::build_application();
    application.run()
}

/// Configure logging. Override verbosity with `RUST_LOG`, e.g.
/// `RUST_LOG=couchcast=trace`.
fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt};
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,couchcast=debug"));
    fmt().with_env_filter(filter).init();
}
