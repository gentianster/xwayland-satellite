use super::super::event::{OutputDimensions, OutputScaleFactor, SurfaceScaleFactor};
use super::*;
use crate::ScalingConfig;
use wayland_client::protocol::wl_region::WlRegion;

// Drain through a protocol sync callback, then flush the notifications generated
// while processing compositor events. Two fixture ticks can leave the final
// output's new mode queued server-side (the earlier outputs get extra ticks).
fn settle(f: &mut TestFixture<FakeXConnection>) {
    let callback = TestObject::<wayland_client::protocol::wl_callback::WlCallback>::from_request(
        &f.xwls_connection.display(),
        wayland_client::protocol::wl_display::Request::Sync {},
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
type Outputs = Vec<(TestObject<WlOutput>, s_proto::wl_output::WlOutput)>;
fn three_outputs(base: Option<f64>) -> (TestFixture<FakeXConnection>, Compositor, Outputs) {
    let mut f = TestFixture::new_pre_connect(|wl| wl.enable_fractional_scale());
    f.satellite.scaling_config = ScalingConfig {
        enabled: true,
        base_scale: base,
    };
    let comp = f.compositor();
    let manager = f.enable_xdg_output();
    let mut outputs = Vec::new();
    for (x, y, w, h, lw, lh, scale) in [
        (0, 720, 3440, 1440, 3440, 1440, 1),
        (3440, 720, 3840, 2160, 2560, 1440, 2),
        (2160, 0, 1920, 1080, 1280, 720, 2),
    ] {
        let (client, output) = f.new_output(x, y);
        f.create_xdg_output(&manager, client.obj.clone());
        output.mode(s_proto::wl_output::Mode::Current, w, h, 144000);
        output.scale(scale);
        f.testwl
            .get_xdg_output(&output)
            .unwrap()
            .logical_size(lw, lh);
        output.done();
        settle(&mut f);
        outputs.push((client, output));
    }
    (f, comp, outputs)
}
fn mode_and_position(output: &TestObject<WlOutput>) -> ((i32, i32), (i32, i32)) {
    let events = output.data.events.lock().unwrap();
    let mode = events
        .iter()
        .rev()
        .find_map(|e| {
            if let Ev::<WlOutput>::Mode { width, height, .. } = e {
                Some((*width, *height))
            } else {
                None
            }
        })
        .unwrap();
    let pos = events
        .iter()
        .rev()
        .find_map(|e| {
            if let Ev::<WlOutput>::Geometry { x, y, .. } = e {
                Some((*x, *y))
            } else {
                None
            }
        })
        .unwrap();
    (pos, mode)
}
#[test]
fn config_is_off_by_default_and_validates_base() {
    assert_eq!(
        ScalingConfig::default(),
        ScalingConfig {
            enabled: false,
            base_scale: None
        }
    );
    for value in [None, Some("0"), Some("false"), Some("off"), Some("")] {
        assert!(!ScalingConfig::from_values(value, Some("1.5")).enabled);
    }
    for (input, want) in [
        ("1", Some(1.0)),
        ("1.5", Some(1.5)),
        ("9", Some(8.0)),
        ("0", Some(1.0)),
        ("NaN", None),
        ("inf", None),
        ("wrong", None),
    ] {
        assert_eq!(
            ScalingConfig::from_values(Some("1"), Some(input)).base_scale,
            want
        );
    }
}
#[test]
fn actual_three_outputs_fixed_and_automatic_layout() {
    for (base, want) in [(Some(1.0), 1.0), (Some(1.5), 1.5), (None, 1.5)] {
        let (f, _, outputs) = three_outputs(base);
        assert_eq!(f.satellite.current_scale, want);
        for (index, (x, y, w, h)) in [
            (0, 720, 3440, 1440),
            (3440, 720, 2560, 1440),
            (2160, 0, 1280, 720),
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(
                mode_and_position(&outputs[index].0),
                (
                    ((x as f64 * want) as i32, (y as f64 * want) as i32),
                    ((w as f64 * want) as i32, (h as f64 * want) as i32)
                )
            );
        }
        let actual = f
            .satellite
            .world
            .query::<&OutputScaleFactor>()
            .iter()
            .map(|(_, s)| s.get())
            .collect::<Vec<_>>();
        assert_eq!(actual.iter().filter(|v| **v == 1.5).count(), 2);
        assert_eq!(actual.iter().filter(|v| **v == 1.0).count(), 1);
    }
}
#[test]
fn surface_enter_and_preference_never_change_global_base() {
    for base in [Some(1.0), Some(1.5), None] {
        let (mut f, comp, outputs) = three_outputs(base);
        let want = base.unwrap_or(1.5);
        let root = Window::new(1);
        let (_, id) = f.create_toplevel(&comp, root);
        for index in [0, 1, 2, 1, 0] {
            f.testwl.move_surface_to_output(id, &outputs[index].1);
            for preferred in [120, 180, 120] {
                f.testwl
                    .get_surface_data(id)
                    .unwrap()
                    .fractional
                    .as_ref()
                    .unwrap()
                    .preferred_scale(preferred);
                f.run();
                f.run();
                assert_eq!(f.satellite.current_scale, want);
                assert_eq!(
                    f.satellite
                        .world
                        .get::<&SurfaceScaleFactor>(f.satellite.windows[&root])
                        .unwrap()
                        .0,
                    want
                );
            }
        }
    }
}
#[test]
fn base_changes_preserve_logical_size_and_wait_for_output_done() {
    let (mut f, comp, outputs) = three_outputs(None);
    let root = Window::new(1);
    let (_, id) = f.create_toplevel(&comp, root);
    f.testwl.move_surface_to_output(id, &outputs[0].1);
    f.run();
    f.run();
    let before = f
        .testwl
        .get_surface_data(id)
        .unwrap()
        .viewport
        .as_ref()
        .unwrap()
        .width;
    // Both 1.5 outputs become 1x; keep the old base until the shared done.
    for (_, output) in &outputs[1..] {
        output.mode(s_proto::wl_output::Mode::Current, 1280, 720, 60000);
        f.testwl
            .get_xdg_output(output)
            .unwrap()
            .logical_size(1280, 720);
    }
    f.run();
    f.run();
    assert_eq!(f.satellite.current_scale, 1.5);
    for (_, output) in &outputs[1..] {
        output.done();
    }
    f.run();
    f.run();
    assert_eq!(f.satellite.current_scale, 1.0);
    assert_eq!(
        f.testwl
            .get_surface_data(id)
            .unwrap()
            .viewport
            .as_ref()
            .unwrap()
            .width,
        before
    );
    assert_eq!(f.connection().window(root).dims.width, 100);
}
#[test]
fn input_region_snapshot_add_subtract_survives_destroy_and_base_change() {
    let (mut f, comp, _) = three_outputs(Some(1.5));
    let root = Window::new(1);
    let (surface, id) = f.create_toplevel(&comp, root);
    let region = TestObject::<WlRegion>::from_request(
        &comp.compositor.obj,
        Req::<WlCompositor>::CreateRegion {},
    );
    region
        .send_request(Req::<WlRegion>::Add {
            x: -30,
            y: -15,
            width: 150,
            height: 120,
        })
        .unwrap();
    region
        .send_request(Req::<WlRegion>::Subtract {
            x: 0,
            y: 0,
            width: 30,
            height: 30,
        })
        .unwrap();
    surface
        .send_request(Req::<WlSurface>::SetInputRegion {
            region: Some(region.obj.clone()),
        })
        .unwrap();
    // wl_surface.set_input_region copies state: later mutation/destruction must not affect it.
    region
        .send_request(Req::<WlRegion>::Add {
            x: 900,
            y: 900,
            width: 9,
            height: 9,
        })
        .unwrap();
    region.obj.destroy();
    surface.send_request(Req::<WlSurface>::Commit).unwrap();
    f.run();
    f.run();
    assert_eq!(
        f.testwl.get_surface_data(id).unwrap().input_region,
        Some(vec![(true, -20, -10, 100, 80), (false, 0, 0, 20, 20)])
    );
    let entity = f.satellite.windows[&root];
    let old = f
        .satellite
        .world
        .get::<&super::super::dispatch::ScaledRegion>(entity)
        .unwrap()
        .client
        .clone()
        .unwrap();
    f.satellite.scaling_config.base_scale = Some(1.0);
    let output = f
        .satellite
        .world
        .query::<&OutputDimensions>()
        .iter()
        .next()
        .unwrap()
        .0;
    f.satellite.updated_outputs.push(output);
    f.run();
    f.run();
    assert!(!old.is_alive());
    assert_eq!(
        f.testwl.get_surface_data(id).unwrap().input_region,
        Some(vec![(true, -30, -15, 150, 120), (false, 0, 0, 30, 30)])
    );
}

#[test]
fn common_base_pointer_coordinates_cursor_and_lock_follow_image_basis() {
    for base in [1.0, 1.5] {
        let (mut f, comp, outputs) = three_outputs(Some(base));
        let pointer =
            TestObject::<WlPointer>::from_request(&comp.seat.obj, wl_seat::Request::GetPointer {});
        let root = Window::new(1);
        let (surface, id) = f.create_toplevel(&comp, root);
        for (_, output) in &outputs {
            f.testwl.move_surface_to_output(id, output);
            f.testwl.move_pointer_to(id, 20.0, 30.0);
            settle(&mut f);
            let events = pointer.data.events.lock().unwrap();
            let (x, y) = events
                .iter()
                .rev()
                .find_map(|e| match e {
                    Ev::<WlPointer>::Enter {
                        surface_x,
                        surface_y,
                        ..
                    } => Some((*surface_x, *surface_y)),
                    Ev::<WlPointer>::Motion {
                        surface_x,
                        surface_y,
                        ..
                    } => Some((*surface_x, *surface_y)),
                    _ => None,
                })
                .unwrap();
            assert_eq!((x, y), (20.0 * base, 30.0 * base));
        }
        let locked = TestObject::<ZwpLockedPointerV1>::from_request(
            &comp.pointer_constraints.obj,
            zwp_pointer_constraints_v1::Request::LockPointer {
                surface: surface.obj.clone(),
                pointer: pointer.obj.clone(),
                region: None,
                lifetime: WEnum::Value(zwp_pointer_constraints_v1::Lifetime::Persistent),
            },
        );
        locked.set_cursor_position_hint(60.0 * base, 90.0 * base);
        settle(&mut f);
        assert_eq!(
            f.testwl.locked_pointer().unwrap().cursor_hint,
            Some(testwl::Vec2f { x: 60.0, y: 90.0 })
        );
        let nominal = super::super::event::cursor_base_size();
        let size = (nominal as f64 * base).round() as i32;
        comp.set_cursor(&pointer, size, size, (12, 18));
        settle(&mut f);
        assert_eq!(
            cursor_viewport(&f),
            if base == 1.0 {
                (-1, -1)
            } else {
                (nominal, nominal)
            }
        );
        assert_eq!(
            f.testwl.cursor_hotspot(),
            Some(((12.0 / base).round() as i32, (18.0 / base).round() as i32))
        );
        // A DPI change cannot redraw a client-owned image; retain its raster
        // basis until the replacement is attached, preserving logical size.
        f.satellite.scaling_config.base_scale = Some(if base == 1.0 { 1.5 } else { 1.0 });
        let output = f
            .satellite
            .world
            .query::<&OutputDimensions>()
            .iter()
            .next()
            .unwrap()
            .0;
        f.satellite.updated_outputs.push(output);
        settle(&mut f);
        assert_eq!(
            cursor_viewport(&f),
            if base == 1.0 {
                (-1, -1)
            } else {
                (nominal, nominal)
            }
        );
        locked.set_cursor_position_hint(
            60.0 * f.satellite.current_scale,
            90.0 * f.satellite.current_scale,
        );
        settle(&mut f);
        assert_eq!(
            f.testwl.locked_pointer().unwrap().cursor_hint,
            Some(testwl::Vec2f { x: 60.0, y: 90.0 })
        );
        let new_base = f.satellite.current_scale;
        let new_size = (nominal as f64 * new_base).round() as i32;
        comp.set_cursor(&pointer, new_size, new_size, (12, 18));
        settle(&mut f);
        assert_eq!(
            cursor_viewport(&f),
            if new_base == 1.0 {
                (-1, -1)
            } else {
                (nominal, nominal)
            }
        );
        assert_eq!(
            f.testwl.cursor_hotspot(),
            Some((
                (12.0 / new_base).round() as i32,
                (18.0 / new_base).round() as i32
            ))
        );
    }
}
#[test]
fn common_base_signed_nested_popup_resize_and_output_relocation() {
    for base in [1.0, 1.5] {
        let (mut f, comp, outputs) = three_outputs(Some(base));
        let root = Window::new(1);
        let (_, root_id) = f.create_toplevel(&comp, root);
        f.testwl.move_surface_to_output(root_id, &outputs[1].1);
        settle(&mut f);
        let root_dims = f.connection().window(root).dims;
        assert_eq!(
            (root_dims.x, root_dims.y),
            ((3440.0 * base) as i16, (720.0 * base) as i16)
        );
        let first = Window::new(2);
        let x = root_dims.x - 30;
        let y = root_dims.y - 30;
        let (_, first_id) = f.create_popup(
            &comp,
            PopupBuilder::new(first, root, root_id)
                .x(x)
                .y(y)
                .width(90)
                .height(90)
                .scale(base),
        );
        f.testwl.move_surface_to_output(first_id, &outputs[0].1);
        settle(&mut f);
        let a = f.connection().window(first).dims;
        assert_eq!((a.x, a.y), (x, y));
        let nested = Window::new(3);
        let (_, nested_id) = f.create_popup(
            &comp,
            PopupBuilder::new(nested, first, first_id)
                .transient_for(first)
                .x(x - 15)
                .y(y - 15)
                .width(30)
                .height(30)
                .scale(base),
        );
        f.testwl.move_surface_to_output(nested_id, &outputs[2].1);
        settle(&mut f);
        let b = f.connection().window(nested).dims;
        assert_eq!((b.x, b.y), (x - 15, y - 15));
        f.reconfigure_window(
            first,
            WindowDims {
                x: x + 45,
                y: y + 45,
                width: 120,
                height: 90,
            },
            true,
        );
        settle(&mut f);
        let a = f.connection().window(first).dims;
        let b = f.connection().window(nested).dims;
        assert_eq!((a.x, a.y, a.width), (x + 45, y + 45, 120));
        assert_eq!((b.x, b.y), (x + 30, y + 30));
        f.testwl.move_output(&outputs[1].1, 3540, 820);
        settle(&mut f);
        let shift = (100.0 * base) as i16;
        assert_eq!(
            (
                f.connection().window(first).dims.x,
                f.connection().window(nested).dims.x
            ),
            (x + 45 + shift, x + 30 + shift)
        );
        let before_a = f.connection().window(first).dims;
        let before_b = f.connection().window(nested).dims;
        let new_base = if base == 1.0 { 1.5 } else { 1.0 };
        f.satellite.scaling_config.base_scale = Some(new_base);
        let output = f
            .satellite
            .world
            .query::<&OutputDimensions>()
            .iter()
            .next()
            .unwrap()
            .0;
        f.satellite.updated_outputs.push(output);
        settle(&mut f);
        let a = f.connection().window(first).dims;
        let b = f.connection().window(nested).dims;
        assert_eq!(
            (a.x, a.y),
            (
                ((before_a.x as f64) * new_base / base).round() as i16,
                ((before_a.y as f64) * new_base / base).round() as i16
            )
        );
        assert_eq!(
            (b.x, b.y),
            (
                ((before_b.x as f64) * new_base / base).round() as i16,
                ((before_b.y as f64) * new_base / base).round() as i16
            )
        );
    }
}
#[test]
fn common_base_android_group_ambiguity_and_explicit_parent_priority() {
    for base in [1.0, 1.5] {
        let (mut f, comp, _) = three_outputs(Some(base));
        let group = Window::new(99);
        let a = Window::new(1);
        let b = Window::new(2);
        for window in [a, b] {
            f.create_toplevel(&comp, window);
            let entity = f.satellite.windows[&window];
            f.satellite
                .world
                .get::<&mut super::super::WindowData>(entity)
                .unwrap()
                .attrs
                .group = Some(group);
        }
        let popup = Window::new(3);
        f.new_window(popup, true, WindowData::default());
        let entity = f.satellite.windows[&popup];
        f.satellite
            .world
            .get::<&mut super::super::WindowData>(entity)
            .unwrap()
            .attrs
            .group = Some(group);
        assert_eq!(f.satellite.popup_parent(popup, None), None);
        assert_eq!(f.satellite.popup_parent(popup, Some(a)), Some(a));
        f.satellite.unmap_window(b);
        settle(&mut f);
        assert_eq!(f.satellite.popup_parent(popup, None), Some(a));
        f.satellite.unmap_window(a);
        settle(&mut f);
        assert_eq!(f.satellite.popup_parent(popup, None), None);
    }
}
#[test]
fn automatic_base_hot_unplug_reprojects_without_changing_logical_size() {
    let (mut f, comp, mut outputs) = three_outputs(None);
    let root = Window::new(1);
    let (_, id) = f.create_toplevel(&comp, root);
    f.testwl.move_surface_to_output(id, &outputs[0].1);
    settle(&mut f);
    let logical = f
        .testwl
        .get_surface_data(id)
        .unwrap()
        .viewport
        .as_ref()
        .unwrap()
        .width;
    let (_, upper) = outputs.pop().unwrap();
    f.remove_output(upper);
    settle(&mut f);
    assert_eq!(f.satellite.current_scale, 1.5);
    let (_, right) = outputs.pop().unwrap();
    f.remove_output(right);
    settle(&mut f);
    assert_eq!(f.satellite.current_scale, 1.0);
    assert_eq!(
        f.testwl
            .get_surface_data(id)
            .unwrap()
            .viewport
            .as_ref()
            .unwrap()
            .width,
        logical
    );
}

#[test]
fn common_base_relative_motion_scales_accelerated_but_keeps_unaccelerated_deltas() {
    use wayland_protocols::wp::relative_pointer::zv1::client::{
        zwp_relative_pointer_manager_v1, zwp_relative_pointer_v1::ZwpRelativePointerV1,
    };
    for base in [1.0, 1.5] {
        let (mut f, comp, _) = three_outputs(Some(base));
        let pointer =
            TestObject::<WlPointer>::from_request(&comp.seat.obj, wl_seat::Request::GetPointer {});
        let relative = TestObject::<ZwpRelativePointerV1>::from_request(
            &comp.relative_pointer_man.obj,
            zwp_relative_pointer_manager_v1::Request::GetRelativePointer {
                pointer: pointer.obj,
            },
        );
        settle(&mut f);
        f.testwl.relative_motion(-2.0, 4.0, -6.0, 8.0);
        settle(&mut f);
        let events = relative.data.events.lock().unwrap();
        match events.last().unwrap() {
            Ev::<ZwpRelativePointerV1>::RelativeMotion {
                dx,
                dy,
                dx_unaccel,
                dy_unaccel,
                ..
            } => assert_eq!(
                (*dx, *dy, *dx_unaccel, *dy_unaccel),
                (-2.0 * base, 4.0 * base, -6.0, 8.0)
            ),
            _ => panic!("missing relative motion"),
        }
    }
}
#[test]
fn locked_region_set_region_snapshot_regenerates_and_destroy_cleans_up() {
    let (mut f, comp, _) = three_outputs(Some(1.5));
    let root = Window::new(1);
    let (surface, _) = f.create_toplevel(&comp, root);
    let pointer =
        TestObject::<WlPointer>::from_request(&comp.seat.obj, wl_seat::Request::GetPointer {});
    let locked = TestObject::<ZwpLockedPointerV1>::from_request(
        &comp.pointer_constraints.obj,
        zwp_pointer_constraints_v1::Request::LockPointer {
            surface: surface.obj,
            pointer: pointer.obj,
            region: None,
            lifetime: WEnum::Value(zwp_pointer_constraints_v1::Lifetime::Persistent),
        },
    );
    let region = TestObject::<WlRegion>::from_request(
        &comp.compositor.obj,
        Req::<WlCompositor>::CreateRegion {},
    );
    region
        .send_request(Req::<WlRegion>::Add {
            x: -30,
            y: 0,
            width: 150,
            height: 90,
        })
        .unwrap();
    region
        .send_request(Req::<WlRegion>::Subtract {
            x: 0,
            y: 0,
            width: 30,
            height: 30,
        })
        .unwrap();
    locked.set_region(Some(&region.obj));
    region.obj.destroy();
    settle(&mut f);
    assert_eq!(
        f.testwl.locked_pointer().unwrap().region,
        Some(vec![(true, -20, 0, 100, 60), (false, 0, 0, 20, 20)])
    );
    f.satellite.scaling_config.base_scale = Some(1.0);
    let e = f
        .satellite
        .world
        .query::<&OutputDimensions>()
        .iter()
        .next()
        .unwrap()
        .0;
    f.satellite.updated_outputs.push(e);
    settle(&mut f);
    assert_eq!(
        f.testwl.locked_pointer().unwrap().region,
        Some(vec![(true, -30, 0, 150, 90), (false, 0, 0, 30, 30)])
    );
    locked.destroy();
    settle(&mut f);
    assert!(f.testwl.locked_pointer().is_none());
    assert_eq!(
        f.satellite
            .world
            .query::<&super::super::dispatch::ScaledRegion>()
            .iter()
            .count(),
        0
    );
}
