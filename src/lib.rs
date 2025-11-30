pub mod ops;

mod app;
pub mod commands;

// Re-export App and constants from app module
pub use app::App;
pub use app::GLOBAL_BRANCH_PREFIX;

// Disable colors for all tests to get clean output
#[cfg(test)]
#[ctor::ctor]
fn init_tests() {
    colored::control::set_override(false);
}
