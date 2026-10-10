//! Platform glue that reaches past eframe and winit into the OS. Each module is built only on its
//! own platform and keeps its unsafe code to itself.

#[cfg(target_os = "macos")]
pub mod mac_traffic_lights;
