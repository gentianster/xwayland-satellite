//! Native content origins, ordered on the same connection as input.
use super::*;
use crate::position_protocol::client::{
    shoji_xwayland_position_manager_v1::{self, ShojiXwaylandPositionManagerV1 as Manager},
    shoji_xwayland_position_v1::{self, ShojiXwaylandPositionV1 as Position},
};
use wayland_client::Dispatch;

#[derive(Clone, Debug)]
pub(super) struct Key {
    pub entity: Entity,
    pub native: client::wl_surface::WlSurface,
}
#[derive(Clone, Copy, Debug)]
pub(super) struct Origin {
    pub x: f64,
    pub y: f64,
    pub geometry_x: i32,
    pub geometry_y: i32,
}
pub(super) struct Tracking {
    pub native: client::wl_surface::WlSurface,
    pub object: Position,
    pub origin: Option<Origin>,
    pending: Option<(u32, Option<Origin>)>,
    committed_epoch: Option<u32>,
}
#[derive(Debug)]
pub(super) enum PositionEvent {
    Surface {
        object: Position,
        native: client::wl_surface::WlSurface,
        event: shoji_xwayland_position_v1::Event,
    },
    Done(u32),
}
impl Dispatch<Manager, Entity> for MyWorld {
    fn event(
        state: &mut Self,
        _: &Manager,
        event: shoji_xwayland_position_manager_v1::Event,
        key: &Entity,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let shoji_xwayland_position_manager_v1::Event::Done { epoch } = event;
        state.push_position_event(*key, PositionEvent::Done(epoch));
    }
}
impl Dispatch<Position, Key> for MyWorld {
    fn event(
        state: &mut Self,
        object: &Position,
        event: shoji_xwayland_position_v1::Event,
        key: &Key,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        state.push_position_event(
            key.entity,
            PositionEvent::Surface {
                object: object.clone(),
                native: key.native.clone(),
                event,
            },
        );
    }
}
impl Event for PositionEvent {
    fn handle<C: XConnection>(self, target: Entity, state: &mut ServerState<C>) {
        match self {
            Self::Surface {
                object,
                native,
                event,
            } => {
                let Ok(mut track) = state.world.get::<&mut Tracking>(target) else {
                    return;
                };
                if !object.is_alive()
                    || !native.is_alive()
                    || track.object != object
                    || track.native != native
                {
                    return;
                }
                let incoming_epoch = match &event {
                    shoji_xwayland_position_v1::Event::State { epoch, .. }
                    | shoji_xwayland_position_v1::Event::Invalidate { epoch } => *epoch,
                };
                if track.committed_epoch.is_some_and(|old| {
                    incoming_epoch.wrapping_sub(old) == 0
                        || incoming_epoch.wrapping_sub(old) >= (1 << 31)
                }) {
                    return;
                }
                if track
                    .pending
                    .is_some_and(|(old, _)| incoming_epoch.wrapping_sub(old) >= (1 << 31))
                {
                    return;
                }
                track.pending = match event {
                    shoji_xwayland_position_v1::Event::State {
                        epoch,
                        x,
                        y,
                        geometry_x,
                        geometry_y,
                    } => Some((
                        epoch,
                        Some(Origin {
                            x,
                            y,
                            geometry_x,
                            geometry_y,
                        }),
                    )),
                    shoji_xwayland_position_v1::Event::Invalidate { epoch } => Some((epoch, None)),
                };
            }
            Self::Done(epoch) => {
                for (_, track) in state.world.query_mut::<&mut Tracking>() {
                    if track.pending.is_some_and(|(e, _)| e == epoch) {
                        track.origin = track.pending.take().unwrap().1;
                        track.committed_epoch = Some(epoch);
                    }
                }
            }
        }
    }
}
impl<S: X11Selection> InnerServerState<S> {
    pub(super) fn track_native_position(&mut self, entity: Entity) {
        let Some(manager) = &self.host_position_manager else {
            return;
        };
        let native = {
            let Ok(native) = self.world.get::<&client::wl_surface::WlSurface>(entity) else {
                return;
            };
            (*native).clone()
        };
        let object = manager.get_surface(
            &native,
            &self.qh,
            Key {
                entity,
                native: native.clone(),
            },
        );
        self.world
            .insert_one(
                entity,
                Tracking {
                    native,
                    object,
                    origin: None,
                    pending: None,
                    committed_epoch: None,
                },
            )
            .unwrap();
    }
    pub(super) fn native_geometry_offset(&self, window: x::Window) -> (i32, i32) {
        self.windows
            .get(&window)
            .and_then(|e| self.world.get::<&Tracking>(*e).ok())
            .and_then(|t| t.origin)
            .map(|o| (o.geometry_x, o.geometry_y))
            .unwrap_or((0, 0))
    }
    pub(super) fn sync_native_positions(&mut self) {
        let base = self.current_scale;
        let min = self.global_output_offset;
        let updates = self
            .world
            .query::<(&Tracking, &WindowData, &client::wl_surface::WlSurface)>()
            .iter()
            .filter_map(|(entity, (track, data, native))| {
                let origin = track.origin?;
                if !native.is_alive() || track.native != *native || !track.object.is_alive() {
                    return None;
                }
                let x = ((origin.x - f64::from(min.x.value)) * base).round() as i16;
                let y = ((origin.y - f64::from(min.y.value)) * base).round() as i16;
                ((x, y) != (data.attrs.dims.x, data.attrs.dims.y)).then_some((entity, x, y))
            })
            .collect::<Vec<_>>();
        for (entity, x, y) in updates {
            let dims = {
                let mut data = self.world.get::<&mut WindowData>(entity).unwrap();
                data.attrs.dims.x = x;
                data.attrs.dims.y = y;
                data.attrs.dims
            };
            self.world
                .insert_one(
                    entity,
                    PendingSurfaceState {
                        x: x.into(),
                        y: y.into(),
                        width: dims.width.into(),
                        height: dims.height.into(),
                    },
                )
                .unwrap();
        }
    }
    pub(super) fn native_layout_ready(&self) -> bool {
        !self
            .world
            .query::<&Tracking>()
            .iter()
            .any(|(_, t)| t.pending.is_some())
    }
}

impl<S: X11Selection> InnerServerState<S> {
    pub(super) fn input_native_surface(
        &self,
        target: Entity,
        event: &ObjectEvent,
    ) -> Option<client::wl_surface::WlSurface> {
        match event {
            ObjectEvent::Pointer(
                client::wl_pointer::Event::Enter { surface, .. }
                | client::wl_pointer::Event::Leave { surface, .. },
            ) => return Some(surface.clone()),
            ObjectEvent::Keyboard(
                client::wl_keyboard::Event::Enter { surface, .. }
                | client::wl_keyboard::Event::Leave { surface, .. },
            ) => return Some(surface.clone()),
            ObjectEvent::Touch(client::wl_touch::Event::Down { surface, .. }) => {
                return Some(surface.clone());
            }
            ObjectEvent::Touch(
                client::wl_touch::Event::Motion { id, .. }
                | client::wl_touch::Event::Up { id, .. }
                | client::wl_touch::Event::Shape { id, .. }
                | client::wl_touch::Event::Orientation { id, .. },
            ) => {
                return self
                    .world
                    .get::<&TouchFocus>(target)
                    .ok()
                    .and_then(|focus| focus.0.get(id).cloned());
            }
            _ => {}
        }
        if let Some((_, _, native)) = self
            .deferred_position_inputs
            .iter()
            .rev()
            .find(|(entity, queued, _)| *entity == target && Self::same_input_focus(queued, event))
        {
            return native.clone();
        }
        let owner = self
            .world
            .get::<&RelatedPointer>(target)
            .map_or(target, |p| p.0);
        if matches!(event, ObjectEvent::Keyboard(_)) {
            if let Ok(focus) = self.world.get::<&NativeKeyboardFocus>(owner) {
                return Some(focus.0.clone());
            }
        } else if let Ok(focus) = self.world.get::<&NativePointerFocus>(owner) {
            return Some(focus.0.clone());
        }
        self.forwarded_input_surface(target, event)
    }
    pub(super) fn forwarded_input_surface(
        &self,
        target: Entity,
        event: &ObjectEvent,
    ) -> Option<client::wl_surface::WlSurface> {
        if let ObjectEvent::Touch(
            client::wl_touch::Event::Motion { id, .. }
            | client::wl_touch::Event::Up { id, .. }
            | client::wl_touch::Event::Shape { id, .. }
            | client::wl_touch::Event::Orientation { id, .. },
        ) = event
        {
            return self
                .world
                .get::<&TouchFocus>(target)
                .ok()
                .and_then(|f| f.0.get(id).cloned());
        }
        if matches!(event, ObjectEvent::Keyboard(_)) {
            return self
                .world
                .get::<&KeyboardFocus>(target)
                .ok()
                .map(|f| f.0.clone());
        }
        let pointer = self
            .world
            .get::<&RelatedPointer>(target)
            .map_or(target, |p| p.0);
        if let Ok(pending) = self.world.get::<&super::event::PendingEnter>(pointer) {
            if let client::wl_pointer::Event::Enter { surface, .. } = &pending.0 {
                return Some(surface.clone());
            }
        }
        let current = self.world.get::<&CurrentSurface>(pointer).ok()?;
        let CurrentSurface::Xwayland(entity) = *current else {
            return None;
        };
        self.world
            .get::<&client::wl_surface::WlSurface>(entity)
            .ok()
            .map(|n| (*n).clone())
    }
    pub(super) fn note_native_input_focus(&mut self, target: Entity, event: &ObjectEvent) {
        match event {
            ObjectEvent::Pointer(client::wl_pointer::Event::Enter { surface, .. }) => {
                let _ = self
                    .world
                    .insert_one(target, NativePointerFocus(surface.clone()));
            }
            ObjectEvent::Pointer(client::wl_pointer::Event::Leave { .. }) => {
                let _ = self.world.remove_one::<NativePointerFocus>(target);
            }
            ObjectEvent::Keyboard(client::wl_keyboard::Event::Enter { surface, .. }) => {
                let _ = self
                    .world
                    .insert_one(target, NativeKeyboardFocus(surface.clone()));
            }
            ObjectEvent::Keyboard(client::wl_keyboard::Event::Leave { .. }) => {
                let _ = self.world.remove_one::<NativeKeyboardFocus>(target);
            }
            _ => {}
        }
    }
    pub(super) fn same_input_focus(a: &ObjectEvent, b: &ObjectEvent) -> bool {
        let keyboard_a = matches!(a, ObjectEvent::Keyboard(_));
        let keyboard_b = matches!(b, ObjectEvent::Keyboard(_));
        if keyboard_a || keyboard_b {
            return keyboard_a && keyboard_b;
        }
        let pointer_a = matches!(a, ObjectEvent::Pointer(_) | ObjectEvent::RelativePointer(_));
        let pointer_b = matches!(b, ObjectEvent::Pointer(_) | ObjectEvent::RelativePointer(_));
        if pointer_a || pointer_b {
            return pointer_a && pointer_b;
        }
        matches!((a, b), (ObjectEvent::Touch(_), ObjectEvent::Touch(_)))
    }
    pub(super) fn input_needs_focus(event: &ObjectEvent) -> bool {
        matches!(
            event,
            ObjectEvent::Pointer(
                client::wl_pointer::Event::Motion { .. }
                    | client::wl_pointer::Event::Button { .. }
                    | client::wl_pointer::Event::Axis { .. }
                    | client::wl_pointer::Event::AxisStop { .. }
            ) | ObjectEvent::Keyboard(client::wl_keyboard::Event::Key { .. })
                | ObjectEvent::RelativePointer(_)
                | ObjectEvent::Touch(
                    client::wl_touch::Event::Motion { .. } | client::wl_touch::Event::Up { .. }
                )
        )
    }
    pub(super) fn note_input_focus(&mut self, target: Entity, event: &ObjectEvent) {
        match event {
            ObjectEvent::Keyboard(client::wl_keyboard::Event::Enter { surface, .. }) => {
                let _ = self
                    .world
                    .insert_one(target, KeyboardFocus(surface.clone()));
            }
            ObjectEvent::Keyboard(client::wl_keyboard::Event::Leave { .. }) => {
                let _ = self.world.remove_one::<KeyboardFocus>(target);
            }
            ObjectEvent::Touch(client::wl_touch::Event::Down { surface, id, .. }) => {
                if !self
                    .world
                    .entity(target)
                    .is_ok_and(|e| e.has::<TouchFocus>())
                {
                    let _ = self.world.insert_one(target, TouchFocus::default());
                }
                if let Ok(mut focus) = self.world.get::<&mut TouchFocus>(target) {
                    focus.0.insert(*id, surface.clone());
                }
            }
            ObjectEvent::Touch(client::wl_touch::Event::Up { id, .. }) => {
                if let Ok(mut focus) = self.world.get::<&mut TouchFocus>(target) {
                    focus.0.remove(id);
                }
            }
            ObjectEvent::Touch(client::wl_touch::Event::Cancel) => {
                let _ = self.world.remove_one::<TouchFocus>(target);
            }
            _ => {}
        }
    }
    pub(super) fn input_origin_available(&self, native: &client::wl_surface::WlSurface) -> bool {
        if !native.is_alive() {
            return false;
        }
        let Some(mut entity) = native.data::<Entity>().copied() else {
            return false;
        };
        if self
            .world
            .get::<&client::wl_surface::WlSurface>(entity)
            .is_err()
        {
            return false;
        }
        if self
            .world
            .get::<&client::wl_surface::WlSurface>(entity)
            .is_ok_and(|n| *n != *native)
        {
            return false;
        }
        let mut visited = HashSet::new();
        while visited.insert(entity) {
            if let Ok(track) = self.world.get::<&Tracking>(entity) {
                return track.origin.is_some()
                    && track.native.is_alive()
                    && track.object.is_alive();
            }
            let next = self
                .world
                .get::<&SurfaceRole>(entity)
                .ok()
                .and_then(|role| match &*role {
                    SurfaceRole::Popup(Some(popup)) => self.windows.get(&popup.parent).copied(),
                    _ => None,
                });
            let Some(next) = next else { return true };
            entity = next;
        }
        false
    }
}

struct KeyboardFocus(client::wl_surface::WlSurface);
#[derive(Default)]
struct TouchFocus(HashMap<i32, client::wl_surface::WlSurface>);
pub(super) struct RelatedPointer(pub Entity);

struct NativePointerFocus(client::wl_surface::WlSurface);
struct NativeKeyboardFocus(client::wl_surface::WlSurface);
