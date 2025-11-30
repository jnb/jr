pub mod ops;

mod app;
pub mod commands;
pub mod config;

// Re-export App and Config from modules
pub use app::App;
pub use config::Config;

// Disable colors for all tests to get clean output
#[cfg(test)]
#[ctor::ctor]
fn init_tests() {
    colored::control::set_override(false);
}
