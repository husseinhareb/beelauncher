//! Wayland surface lifecycle, layer-shell overlay, and shared-memory buffers.
//!
//! Architecture:
//!   - One WlSurface committed as a layer-shell overlay (zwlr_layer_shell_v1).
//!   - Double-buffered shared-memory via SmithaySlotPool for tear-free presents.
//!   - Input arrives via wl_keyboard; xkbcommon decoding is handled by smithay.
//!   - Blocking event dispatch drives both protocol events and app logic.

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_keyboard, delegate_layer, delegate_output, delegate_registry,
    delegate_seat, delegate_shm,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        keyboard::{KeyboardHandler, KeyEvent, Keysym, Modifiers, RepeatInfo},
        Capability, SeatHandler, SeatState,
    },
    shell::{
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
        WaylandSurface,
    },
    shm::{slot::SlotPool, Shm, ShmHandler},
};
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_seat, wl_surface, wl_shm},
    Connection, QueueHandle,
};

use crate::{
    app::AppState,
    config::Config,
    input,
};

// ── Entry point ───────────────────────────────────────────────────────────────

/// Run the launcher event loop. Returns when the user dismisses the launcher.
pub fn run(cfg: Config) -> anyhow::Result<()> {
    let conn = Connection::connect_to_env()?;
    let (globals, mut event_queue) = registry_queue_init(&conn)?;
    let qh: QueueHandle<WaylandState> = event_queue.handle();

    let compositor  = CompositorState::bind(&globals, &qh)?;
    let layer_shell = LayerShell::bind(&globals, &qh)?;
    let shm         = Shm::bind(&globals, &qh)?;

    let surface = compositor.create_surface(&qh);

    let layer_surface = layer_shell.create_layer_surface(
        &qh,
        surface,
        Layer::Overlay,
        Some("beelauncher"),
        None,
    );

    let app_state = AppState::new(&cfg);
    let w = app_state.renderer.layout.width;
    let h = app_state.renderer.layout.total_height;

    configure_layer_surface(&layer_surface, &cfg, w, h);
    layer_surface.commit();

    // Allocate enough for two frames to allow double-buffering.
    let pool = SlotPool::new((w * h * 4 * 2) as usize, &shm)?;

    let mut state = WaylandState {
        registry:       RegistryState::new(&globals),
        seat:           SeatState::new(&globals, &qh),
        output:         OutputState::new(&globals, &qh),
        shm,
        pool,
        layer_surface,
        app:            app_state,
        cfg,
        width:          w,
        height:         h,
        configured:     false,
        dirty:          true,
        modifiers:      Modifiers::default(),
        exit:           false,
    };

    loop {
        event_queue.blocking_dispatch(&mut state)?;

        if state.dirty && state.configured {
            state.draw(&qh);
            state.dirty = false;
        }

        if state.exit || state.app.should_exit {
            break;
        }
    }

    Ok(())
}

fn configure_layer_surface(ls: &LayerSurface, cfg: &Config, w: u32, h: u32) {
    use crate::config::Anchor as CA;

    ls.set_size(w, h);
    ls.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);

    let anchor = match cfg.window.anchor {
        CA::Top    => Anchor::TOP,
        CA::Bottom => Anchor::BOTTOM,
        CA::Center => Anchor::empty(),
    };
    ls.set_anchor(anchor);
    ls.set_margin(cfg.window.margin_top, 0, 0, 0);
}

// ── State ────────────────────────────────────────────────────────────────────

struct WaylandState {
    registry:      RegistryState,
    seat:          SeatState,
    output:        OutputState,
    shm:           Shm,
    pool:          SlotPool,
    layer_surface: LayerSurface,
    app:           AppState,
    cfg:           Config,
    width:         u32,
    height:        u32,
    configured:    bool,
    dirty:         bool,
    modifiers:     Modifiers,
    exit:          bool,
}

impl WaylandState {
    fn draw(&mut self, _qh: &QueueHandle<Self>) {
        let buf_len = (self.width * self.height * 4) as usize;
        let (buffer, canvas) = self.pool
            .create_buffer(
                self.width as i32,
                self.height as i32,
                (self.width * 4) as i32,
                wl_shm::Format::Argb8888,
            )
            .expect("shm buffer allocation");

        // SAFETY: canvas is u8 slice from wl_shm, guaranteed 4-byte aligned,
        // length is width * height * 4 (a multiple of 4).
        let pixels = unsafe {
            std::slice::from_raw_parts_mut(
                canvas.as_mut_ptr() as *mut u32,
                buf_len / 4,
            )
        };

        self.app.render_frame(pixels);

        let surface = self.layer_surface.wl_surface();
        surface.attach(Some(buffer.wl_buffer()), 0, 0);
        surface.damage_buffer(0, 0, self.width as i32, self.height as i32);
        surface.commit();
    }
}

// ── smithay delegate impls ────────────────────────────────────────────────────

impl CompositorHandler for WaylandState {
    fn scale_factor_changed(
        &mut self, _: &Connection, _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface, _: i32,
    ) {}
    fn transform_changed(
        &mut self, _: &Connection, _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface, _: wl_output::Transform,
    ) {}
    fn frame(
        &mut self, _: &Connection, _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface, _: u32,
    ) {}
    fn surface_enter(
        &mut self, _: &Connection, _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface, _: &wl_output::WlOutput,
    ) {}
    fn surface_leave(
        &mut self, _: &Connection, _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface, _: &wl_output::WlOutput,
    ) {}
}

impl OutputHandler for WaylandState {
    fn output_state(&mut self) -> &mut OutputState { &mut self.output }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl LayerShellHandler for WaylandState {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        self.exit = true;
    }
    fn configure(
        &mut self,
        _: &Connection,
        _qh: &QueueHandle<Self>,
        _: &LayerSurface,
        _configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        self.configured = true;
        self.dirty = true;
    }
}

impl SeatHandler for WaylandState {
    fn seat_state(&mut self) -> &mut SeatState { &mut self.seat }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard {
            // get_keyboard returns wl_keyboard::WlKeyboard directly.
            let _ = self.seat.get_keyboard(qh, &seat, None);
        }
    }

    fn remove_capability(
        &mut self, _: &Connection, _: &QueueHandle<Self>,
        _: wl_seat::WlSeat, _: Capability,
    ) {}
}

impl KeyboardHandler for WaylandState {
    fn enter(
        &mut self, _: &Connection, _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface, _: u32,
        _: &[u32], _: &[Keysym],
    ) {}

    fn leave(
        &mut self, _: &Connection, _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface, _: u32,
    ) {}

    fn press_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _kbd: &wl_keyboard::WlKeyboard,
        _serial: u32,
        event: KeyEvent,
    ) {
        if let Some(action) = input::translate(&event, &self.modifiers) {
            let cfg = self.cfg.clone();
            if self.app.handle_action(action, &cfg) {
                self.dirty = true;
            }
            if self.app.should_exit {
                self.exit = true;
            }
        }
    }

    fn release_key(
        &mut self, _: &Connection, _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32, _: KeyEvent,
    ) {}

    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _serial: u32,
        modifiers: Modifiers,
        _layout: u32,
    ) {
        self.modifiers = modifiers;
    }

    fn update_repeat_info(
        &mut self, _: &Connection, _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: RepeatInfo,
    ) {}
}

impl ShmHandler for WaylandState {
    fn shm_state(&mut self) -> &mut Shm { &mut self.shm }
}

impl ProvidesRegistryState for WaylandState {
    fn registry(&mut self) -> &mut RegistryState { &mut self.registry }
    registry_handlers![OutputState, SeatState];
}

delegate_compositor!(WaylandState);
delegate_output!(WaylandState);
delegate_layer!(WaylandState);
delegate_seat!(WaylandState);
delegate_keyboard!(WaylandState);
delegate_shm!(WaylandState);
delegate_registry!(WaylandState);
