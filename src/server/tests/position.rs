use super::super::{Entity, MyWorld, client, position::Tracking};
use super::*;
use crate::position_protocol::client::shoji_xwayland_position_manager_v1::ShojiXwaylandPositionManagerV1;
use wayland_client::Dispatch;

// A fixture tick advances only part of native -> bridge -> Xwayland delivery.
// Drain a sync plus its resulting events before testing absence of input; an
// old pointer Frame would otherwise make an empty-event assertion misleading.
fn settle(f: &mut TestFixture<FakeXConnection>) {
    let callback = TestObject::<wayland_client::protocol::wl_callback::WlCallback>::from_request(
        &f.xwls_connection.display(),
        Req::<WlDisplay>::Sync {},
    );
    for _ in 0..8 {
        f.run();
        if !callback.data.events.lock().unwrap().is_empty() {
            f.run();
            f.run();
            return;
        }
    }
    panic!("isolated protocol sync did not complete");
}
fn fixture() -> (
    TestFixture<FakeXConnection>,
    Compositor,
    TestObject<WlPointer>,
) {
    let mut f = TestFixture::new_pre_connect(|wl| wl.enable_position_protocol());
    f.satellite.scaling_config = crate::ScalingConfig {
        enabled: true,
        base_scale: Some(1.5),
    };
    let key = f.satellite.world.spawn(());
    let manager = f
        .satellite
        .world
        .global_list
        .bind::<ShojiXwaylandPositionManagerV1, _, _>(&f.satellite.qh, 1..=1, key)
        .unwrap();
    f.satellite.host_position_manager = Some(manager);
    let comp = f.compositor();
    let pointer =
        TestObject::<WlPointer>::from_request(&comp.seat.obj, wl_seat::Request::GetPointer {});
    f.new_output(0, 0);
    settle(&mut f);
    assert_eq!(f.satellite.current_scale, 1.5);
    (f, comp, pointer)
}
fn origin(f: &mut TestFixture<FakeXConnection>, id: testwl::SurfaceId, epoch: u32, x: f64, y: f64) {
    assert!(f.testwl.position_is_tracked(id));
    f.testwl.send_surface_position(id, epoch, x, y, 0, 0);
    f.testwl.position_done(epoch);
    settle(f);
}
fn native(f: &TestFixture<FakeXConnection>, window: Window) -> WlSurface {
    (*f.satellite
        .world
        .get::<&WlSurface>(f.satellite.windows[&window])
        .unwrap())
    .clone()
}
fn pointer_event(f: &mut TestFixture<FakeXConnection>, event: client::wl_pointer::Event) {
    let (key, pointer) = f
        .satellite
        .world
        .query::<&WlPointer>()
        .iter()
        .next()
        .map(|(e, p)| (e, p.clone()))
        .unwrap();
    let qh = f.satellite.qh.clone();
    <MyWorld as Dispatch<WlPointer, Entity>>::event(
        &mut f.satellite.world,
        &pointer,
        event,
        &key,
        &f.xwls_connection,
        &qh,
    );
}
fn enter(f: &mut TestFixture<FakeXConnection>, window: Window) {
    let surface = native(f, window);
    pointer_event(
        f,
        client::wl_pointer::Event::Enter {
            serial: 30,
            surface,
            surface_x: 10.0,
            surface_y: 20.0,
        },
    );
}
fn motion(f: &mut TestFixture<FakeXConnection>) {
    pointer_event(
        f,
        client::wl_pointer::Event::Motion {
            time: 31,
            surface_x: 11.0,
            surface_y: 21.0,
        },
    );
}
fn button(f: &mut TestFixture<FakeXConnection>) {
    pointer_event(
        f,
        client::wl_pointer::Event::Button {
            serial: 32,
            time: 32,
            button: 0x110,
            state: WEnum::Value(client::wl_pointer::ButtonState::Pressed),
        },
    );
}
fn take(pointer: &TestObject<WlPointer>) -> Vec<Ev<WlPointer>> {
    std::mem::take(&mut *pointer.data.events.lock().unwrap())
}

#[test]
fn state_requires_matching_done_before_configure_and_input() {
    let (mut f, comp, pointer) = fixture();
    let window = Window::new(1);
    let (_, id) = f.create_toplevel(&comp, window);
    origin(&mut f, id, 1, 0.0, 0.0);
    enter(&mut f, window);
    settle(&mut f);
    take(&pointer);
    f.testwl.send_surface_position(id, 2, 200.0, 100.0, 0, 0);
    settle(&mut f);
    motion(&mut f);
    button(&mut f);
    settle(&mut f);
    assert_eq!(f.connection().window(window).dims.x, 0);
    assert!(take(&pointer).is_empty());
    f.testwl.position_done(1);
    settle(&mut f);
    assert!(
        take(&pointer).is_empty(),
        "a stale done must not release input"
    );
    f.testwl.position_done(2);
    settle(&mut f);
    assert_eq!(
        (
            f.connection().window(window).dims.x,
            f.connection().window(window).dims.y
        ),
        (300, 150)
    );
    let events = take(&pointer);
    assert!(events.iter().any(|e| matches!(
        e,
        Ev::<WlPointer>::Motion {
            surface_x: 16.5,
            surface_y: 31.5,
            ..
        }
    )));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Ev::<WlPointer>::Button { .. }))
    );
}

#[test]
fn leave_does_not_release_a_button_before_done() {
    let (mut f, comp, pointer) = fixture();
    let window = Window::new(1);
    let (_, id) = f.create_toplevel(&comp, window);
    origin(&mut f, id, 1, 0.0, 0.0);
    enter(&mut f, window);
    settle(&mut f);
    take(&pointer);
    f.testwl.send_surface_position(id, 2, 200.0, 100.0, 0, 0);
    settle(&mut f);
    button(&mut f);
    let surface = native(&f, window);
    pointer_event(
        &mut f,
        client::wl_pointer::Event::Leave {
            serial: 33,
            surface,
        },
    );
    settle(&mut f);
    assert!(
        !take(&pointer)
            .iter()
            .any(|e| matches!(e, Ev::<WlPointer>::Button { .. })),
        "Leave cannot bypass the checked geometry barrier"
    );
    assert_eq!(f.connection().window(window).dims.x, 0);
    f.testwl.position_done(2);
    settle(&mut f);
    assert_eq!(f.connection().window(window).dims.x, 300);
}

#[test]
fn popup_pending_enter_keeps_deferred_motion_and_button() {
    let (mut f, comp, pointer) = fixture();
    let root = Window::new(1);
    let (_, root_id) = f.create_toplevel(&comp, root);
    origin(&mut f, root_id, 1, 0.0, 0.0);
    let popup = Window::new(2);
    let (_, id) = f.create_popup(
        &comp,
        PopupBuilder::new(popup, root, root_id)
            .transient_for(root)
            .scale(1.5),
    );
    settle(&mut f);
    take(&pointer);
    f.testwl
        .send_surface_position(root_id, 2, 100.0, 100.0, 0, 0);
    settle(&mut f);
    enter(&mut f, popup);
    motion(&mut f);
    button(&mut f);
    settle(&mut f);
    assert!(take(&pointer).is_empty());
    f.testwl.position_done(2);
    settle(&mut f);
    let events = take(&pointer);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Ev::<WlPointer>::Enter { .. })),
        "popup enter must replay before its input: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Ev::<WlPointer>::Motion { .. })),
        "deferred popup motion lost: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Ev::<WlPointer>::Button { .. })),
        "deferred popup click lost: {events:?}"
    );
    assert!(f.testwl.get_surface_data(id).is_some());
}

#[test]
fn invalidated_and_uninitialized_origins_reject_input() {
    let (mut f, comp, pointer) = fixture();
    let window = Window::new(1);
    let (_, id) = f.create_toplevel(&comp, window);
    settle(&mut f);
    take(&pointer);
    enter(&mut f, window);
    motion(&mut f);
    button(&mut f);
    settle(&mut f);
    assert!(take(&pointer).is_empty());
    origin(&mut f, id, 1, 0.0, 0.0);
    enter(&mut f, window);
    settle(&mut f);
    take(&pointer);
    f.testwl.invalidate_surface_position(id, 2);
    f.testwl.position_done(2);
    settle(&mut f);
    motion(&mut f);
    button(&mut f);
    settle(&mut f);
    assert!(
        take(&pointer).is_empty(),
        "invalidated origin cannot use the old X frame"
    );
}

#[test]
fn rejected_x_configure_holds_input_until_successful_retry() {
    let (mut f, comp, pointer) = fixture();
    let window = Window::new(1);
    let (_, id) = f.create_toplevel(&comp, window);
    origin(&mut f, id, 1, 0.0, 0.0);
    enter(&mut f, window);
    settle(&mut f);
    take(&pointer);
    f.satellite.connection.reject_window_dims = true;
    f.testwl.send_surface_position(id, 2, 200.0, 100.0, 0, 0);
    f.testwl.position_done(2);
    settle(&mut f);
    motion(&mut f);
    button(&mut f);
    settle(&mut f);
    assert_eq!(f.connection().window(window).dims.x, 0);
    assert!(take(&pointer).is_empty());
    f.satellite.connection.reject_window_dims = false;
    settle(&mut f);
    assert_eq!(f.connection().window(window).dims.x, 300);
    assert!(
        take(&pointer)
            .iter()
            .any(|e| matches!(e, Ev::<WlPointer>::Button { .. }))
    );
}

#[test]
fn old_position_object_cannot_update_rebound_native_lifetime() {
    use super::super::{Event, position::PositionEvent};
    use crate::position_protocol::client::shoji_xwayland_position_v1::Event as Position;
    let (mut f, comp, _) = fixture();
    let window = Window::new(1);
    let (_, id) = f.create_toplevel(&comp, window);
    origin(&mut f, id, 1, 10.0, 20.0);
    let entity = f.satellite.windows[&window];
    let old_native = native(&f, window);
    let old_object = f
        .satellite
        .world
        .get::<&Tracking>(entity)
        .unwrap()
        .object
        .clone();
    // A replacement resource on the same entity must not accept events from
    // the former position object, even while both proxies are still alive.
    f.satellite.track_native_position(entity);
    PositionEvent::Surface {
        object: old_object,
        native: old_native,
        event: Position::State {
            epoch: 9,
            x: 999.0,
            y: 999.0,
            geometry_x: 0,
            geometry_y: 0,
        },
    }
    .handle(entity, &mut f.satellite);
    PositionEvent::Done(9).handle(entity, &mut f.satellite);
    assert!(
        f.satellite
            .world
            .get::<&Tracking>(entity)
            .unwrap()
            .origin
            .is_none()
    );
}

#[test]
fn keyboard_focus_uses_the_same_position_barrier() {
    let (mut f, comp, _) = fixture();
    let window = Window::new(1);
    let (_, id) = f.create_toplevel(&comp, window);
    settle(&mut f);
    assert_eq!(
        f.connection().focused_window,
        None,
        "uninitialized native origin cannot focus an old X frame"
    );
    origin(&mut f, id, 1, 0.0, 0.0);
    f.testwl.focus_toplevel(id);
    settle(&mut f);
    assert_eq!(f.connection().focused_window, Some(window));
    f.satellite.connection.focused_window = None;
    f.testwl.send_surface_position(id, 2, 100.0, 50.0, 0, 0);
    settle(&mut f);
    f.testwl.focus_toplevel(id);
    settle(&mut f);
    assert_eq!(f.connection().focused_window, None);
    f.testwl.position_done(2);
    settle(&mut f);
    assert_eq!(f.connection().focused_window, Some(window));
    assert_eq!(f.connection().window(window).dims.x, 150);
}

#[test]
fn older_epochs_cannot_replace_committed_or_pending_origin_across_wrap() {
    let (mut f, comp, _) = fixture();
    let window = Window::new(1);
    let (_, id) = f.create_toplevel(&comp, window);
    origin(&mut f, id, u32::MAX - 1, 10.0, 20.0);
    origin(&mut f, id, 0, 20.0, 30.0);
    assert_eq!(f.connection().window(window).dims.x, 30);
    origin(&mut f, id, u32::MAX, 999.0, 999.0);
    assert_eq!(
        f.connection().window(window).dims.x,
        30,
        "pre-wrap state is older than committed epoch zero"
    );
    f.testwl.send_surface_position(id, 2, 40.0, 50.0, 0, 0);
    f.testwl.send_surface_position(id, 1, 999.0, 999.0, 0, 0);
    f.testwl.position_done(1);
    settle(&mut f);
    assert_eq!(
        f.connection().window(window).dims.x,
        30,
        "older state/done cannot replace a newer pending origin"
    );
    f.testwl.position_done(2);
    settle(&mut f);
    assert_eq!(f.connection().window(window).dims.x, 60);
}

#[test]
fn old_native_surface_cannot_update_reused_entity() {
    use super::super::{Event, position::PositionEvent};
    use crate::position_protocol::client::shoji_xwayland_position_v1::Event as Position;
    let (mut f, comp, _) = fixture();
    let window = Window::new(1);
    let (_, id) = f.create_toplevel(&comp, window);
    origin(&mut f, id, 1, 10.0, 20.0);
    let entity = f.satellite.windows[&window];
    let old_native = native(&f, window);
    let replacement = f
        .satellite
        .compositor
        .create_surface(&f.satellite.qh, entity);
    f.satellite.world.insert_one(entity, replacement).unwrap();
    f.satellite.track_native_position(entity);
    let current_object = f
        .satellite
        .world
        .get::<&Tracking>(entity)
        .unwrap()
        .object
        .clone();
    // Even an event with the replacement position object cannot carry an old
    // native wl_surface into an entity whose lifetime has since changed.
    PositionEvent::Surface {
        object: current_object,
        native: old_native,
        event: Position::State {
            epoch: 9,
            x: 999.0,
            y: 999.0,
            geometry_x: 0,
            geometry_y: 0,
        },
    }
    .handle(entity, &mut f.satellite);
    PositionEvent::Done(9).handle(entity, &mut f.satellite);
    assert!(
        f.satellite
            .world
            .get::<&Tracking>(entity)
            .unwrap()
            .origin
            .is_none()
    );
}

#[test]
fn pointer_leave_preserves_keyboard_focus_waiting_for_done() {
    let (mut f, comp, _) = fixture();
    let first = Window::new(1);
    let (_, first_id) = f.create_toplevel(&comp, first);
    origin(&mut f, first_id, 1, 0.0, 0.0);
    let second = Window::new(2);
    let (_, second_id) = f.create_toplevel(&comp, second);
    origin(&mut f, second_id, 2, 100.0, 100.0);
    f.testwl.focus_toplevel(first_id);
    enter(&mut f, first);
    settle(&mut f);
    f.testwl
        .send_surface_position(second_id, 3, 200.0, 200.0, 0, 0);
    settle(&mut f);
    f.testwl.focus_toplevel(second_id);
    settle(&mut f);
    let surface = native(&f, first);
    pointer_event(
        &mut f,
        client::wl_pointer::Event::Leave {
            serial: 50,
            surface,
        },
    );
    settle(&mut f);
    f.testwl.position_done(3);
    settle(&mut f);
    assert_eq!(
        f.connection().focused_window,
        Some(second),
        "pointer Leave and keyboard Enter share a seat entity, but keyboard focus must survive the pointer cancellation"
    );
}

#[test]
fn keyboard_root_and_pointer_popup_keep_separate_native_focus() {
    let (mut f, comp, pointer) = fixture();
    let keyboard =
        TestObject::<WlKeyboard>::from_request(&comp.seat.obj, wl_seat::Request::GetKeyboard {});
    let root = Window::new(1);
    let (_, root_id) = f.create_toplevel(&comp, root);
    origin(&mut f, root_id, 1, 0.0, 0.0);
    let popup = Window::new(2);
    f.create_popup(
        &comp,
        PopupBuilder::new(popup, root, root_id)
            .transient_for(root)
            .scale(1.5),
    );
    enter(&mut f, popup);
    motion(&mut f);
    settle(&mut f);
    take(&pointer);
    f.testwl.focus_toplevel(root_id);
    settle(&mut f);
    motion(&mut f);
    button(&mut f);
    settle(&mut f);
    let pointer_survived = take(&pointer)
        .iter()
        .any(|e| matches!(e, Ev::<WlPointer>::Button { .. }));

    // Reverse the order: keyboard still focuses the root when pointer enters
    // the popup, so a root key must not inherit the popup's native surface.
    enter(&mut f, popup);
    motion(&mut f);
    settle(&mut f);
    keyboard.data.events.lock().unwrap().clear();
    let (key, native_keyboard) = f
        .satellite
        .world
        .query::<&WlKeyboard>()
        .iter()
        .next()
        .map(|(e, k)| (e, k.clone()))
        .unwrap();
    let qh = f.satellite.qh.clone();
    <MyWorld as Dispatch<WlKeyboard, Entity>>::event(
        &mut f.satellite.world,
        &native_keyboard,
        client::wl_keyboard::Event::Key {
            serial: 60,
            time: 60,
            key: 30,
            state: WEnum::Value(client::wl_keyboard::KeyState::Pressed),
        },
        &key,
        &f.xwls_connection,
        &qh,
    );
    settle(&mut f);
    let keyboard_survived = keyboard
        .data
        .events
        .lock()
        .unwrap()
        .iter()
        .any(|e| matches!(e, Ev::<WlKeyboard>::Key { .. }));
    assert_eq!(
        (pointer_survived, keyboard_survived),
        (true, true),
        "keyboard root focus and pointer popup focus cannot overwrite each other's native input surface"
    );
}
