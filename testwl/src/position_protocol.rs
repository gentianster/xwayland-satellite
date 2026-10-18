//! Server side of the exact protocol XML used by the bridge.
#![allow(non_camel_case_types, non_upper_case_globals)]
use self::__interfaces::*;
use wayland_server::{self, protocol::*};
pub mod __interfaces {
    use wayland_server::backend as wayland_backend;
    use wayland_server::protocol::__interfaces::*;
    wayland_scanner::generate_interfaces!("../protocols/shoji-xwayland-position-v1.xml");
}
wayland_scanner::generate_server_code!("../protocols/shoji-xwayland-position-v1.xml");
