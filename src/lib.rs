pub mod git;
pub mod github;
pub mod jujutsu;

mod app;
pub mod commands;

// Re-export App from app module
pub use app::App;

// Disable colors for all tests to get clean output
#[cfg(test)]
#[ctor::ctor]
fn init_tests() {
    colored::control::set_override(false);
}
