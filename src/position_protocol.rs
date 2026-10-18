//! Optional ShojiWM position extension, shared by the bridge and compositor.
#![allow(non_camel_case_types, non_upper_case_globals)]
pub mod client {
    use wayland_client::{self, protocol::*};
    pub mod __interfaces {
        use wayland_client::backend as wayland_backend;
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("protocols/shoji-xwayland-position-v1.xml");
    }
    use self::__interfaces::*;
    wayland_scanner::generate_client_code!("protocols/shoji-xwayland-position-v1.xml");
}
pub mod server {
    use self::__interfaces::*;
    pub use super::client::__interfaces;
    use wayland_server::{self, protocol::*};
    wayland_scanner::generate_server_code!("protocols/shoji-xwayland-position-v1.xml");
}
