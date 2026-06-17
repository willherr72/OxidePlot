#[cfg(not(target_arch = "wasm32"))]
pub mod loader;
#[cfg(not(target_arch = "wasm32"))]
pub mod parser;
#[cfg(not(target_arch = "wasm32"))]
pub mod datetime;
#[cfg(not(target_arch = "wasm32"))]
pub mod unit_inference;
