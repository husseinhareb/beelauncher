//! Wayland surface lifecycle, layer-shell overlay, and shared-memory buffers.
//!
//! Architecture:
//!   - One WlSurface committed as a layer-shell overlay (zwlr_layer_shell_v1).
//!   - Double-buffered shared-memory via SmithaySlotPool for tear-free presents.
//!   - Input arrives via wl_keyboard and wl_pointer.
//!   - Poll-based event loop with a repeat timer for held navigation/backspace keys.

use std::{
    os::fd::AsFd,
    os::unix::io::AsRawFd,
    time::{Duration, Instant},
};

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_keyboard, delegate_layer, delegate_output, delegate_pointer,
    delegate_registry, delegate_seat, delegate_shm,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        keyboard::{KeyboardHandler, KeyEvent, Keysym, Modifiers, RepeatInfo},
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
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
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
    Connection, QueueHandle,
};

use crate::{
    app::AppState,
    cli::Options,
    config::Config,
    input::{self, Action},
};

const BTN_LEFT:  u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;

// ── Entry point ───────────────────────────────────────────────────────────────

/// Run the launcher event loop. Returns when the user dismisses the launcher.
pub fn run(cfg: Config, opts: Options) -> anyhow::Result<()> {
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

    let app_state = AppState::new(&cfg, &opts);
    let w = app_state.renderer.layout.width;
    let h = app_state.renderer.layout.total_height;

    configure_layer_surface(&layer_surface, &cfg, w, h);
    layer_surface.commit();

    // Allocate enough for two frames to allow double-buffering.
    let pool = SlotPool::new((w * h * 4 * 2) as usize, &shm)?;

    let conn_fd = conn.as_fd().as_raw_fd();

    let mut state = WaylandState {
        registry:           RegistryState::new(&globals),
        seat:               SeatState::new(&globals, &qh),
        output:             OutputState::new(&globals, &qh),
        shm,
        pool,
        layer_surface,
        app:                app_state,
        cfg,
        width:              w,
        height:             h,
        configured:         false,
        dirty:              true,
        modifiers:          Modifiers::default(),
        exit:               false,
        repeat_info:        RepeatInfo::Disable,
        repeat_key:         None,
        repeat_next:        None,
        cursor_pos:         None,
        scroll_accumulator: 0.0,
    };

    loop {
        event_queue.flush()?;

        let timeout_ms: i32 = match state.repeat_next {
            None    => -1,
            Some(t) => {
                let now = Instant::now();
                if now >= t { 0 }
                else { (t - now).as_millis().min(i32::MAX as u128) as i32 }
            }
        };

        if let Some(guard) = event_queue.prepare_read() {
            unsafe {
                let mut pfd = libc::pollfd { fd: conn_fd, events: libc::POLLIN, revents: 0 };
                libc::poll(&mut pfd, 1, timeout_ms);
            }
            let _ = guard.read();
        }

        event_queue.dispatch_pending(&mut state)?;

        // Fire key repeat if its deadline has passed.
        if let Some(next) = state.repeat_next {
            if Instant::now() >= next {
                if let Some(key) = state.repeat_key.clone() {
                    let cfg = state.cfg.clone();
                    if let Some(action) = input::translate(&key, &state.modifiers) {
                        if state.app.handle_action(action, &cfg) {
                            state.dirty = true;
                        }
                    }
                }
                state.repeat_next = Some(Instant::now() + state.repeat_interval());
            }
        }

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
    registry:           RegistryState,
    seat:               SeatState,
    output:             OutputState,
    shm:                Shm,
    pool:               SlotPool,
    layer_surface:      LayerSurface,
    app:                AppState,
    cfg:                Config,
    width:              u32,
    height:             u32,
    configured:         bool,
    dirty:              bool,
    modifiers:          Modifiers,
    exit:               bool,
    repeat_info:        RepeatInfo,
    repeat_key:         Option<KeyEvent>,
    repeat_next:        Option<Instant>,
    /// Last known pointer position in surface-local coordinates.
    cursor_pos:         Option<(f64, f64)>,
    /// Accumulated smooth-scroll delta (resets on discrete scroll events).
    scroll_accumulator: f64,
}

impl WaylandState {
    fn repeat_delay(&self) -> Duration {
        match self.repeat_info {
            RepeatInfo::Repeat { delay, .. } => Duration::from_millis(delay as u64),
            RepeatInfo::Disable              => Duration::MAX,
        }
    }

    fn repeat_interval(&self) -> Duration {
        match self.repeat_info {
            RepeatInfo::Repeat { rate, .. } => Duration::from_millis(1000 / rate.get() as u64),
            RepeatInfo::Disable             => Duration::MAX,
        }
    }

    /// Return the result-list index at surface position `pos`, or None if the
    /// position is over the input field or past the last result.
    fn hit_row(&self, pos: (f64, f64)) -> Option<usize> {
        let layout = &self.app.renderer.layout;
        let y = pos.1 as u32;
        if y < layout.input_height { return None; }
        let row = ((y - layout.input_height) / layout.row_height) as usize;
        let idx = self.app.scroll + row;
        if idx < self.app.results.len() { Some(idx) } else { None }
    }

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
            let _ = self.seat.get_keyboard(qh, &seat, None);
        }
        if capability == Capability::Pointer {
            let _ = self.seat.get_pointer(qh, &seat);
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
    ) {
        self.repeat_key  = None;
        self.repeat_next = None;
    }

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

            if matches!(action, Action::SelectNext | Action::SelectPrev | Action::Backspace) {
                let delay = self.repeat_delay();
                self.repeat_key  = Some(event);
                self.repeat_next = Some(Instant::now() + delay);
            } else {
                self.repeat_key  = None;
                self.repeat_next = None;
            }
        } else {
            self.repeat_key  = None;
            self.repeat_next = None;
        }
    }

    fn release_key(
        &mut self, _: &Connection, _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        if self.repeat_key.as_ref().map(|k| k.raw_code) == Some(event.raw_code) {
            self.repeat_key  = None;
            self.repeat_next = None;
        }
    }

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
        info: RepeatInfo,
    ) {
        self.repeat_info = info;
    }
}

impl PointerHandler for WaylandState {
    fn pointer_frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _pointer: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            match &event.kind {
                // Track position and highlight the row under the cursor.
                PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => {
                    self.cursor_pos = Some(event.position);
                    if let Some(idx) = self.hit_row(event.position) {
                        if self.app.selected != idx {
                            self.app.selected = idx;
                            self.dirty = true;
                        }
                    }
                }

                PointerEventKind::Leave { .. } => {
                    self.cursor_pos = None;
                }

                PointerEventKind::Press { button, .. } => match *button {
                    BTN_LEFT => {
                        // Select the clicked row then confirm (launch).
                        if let Some(pos) = self.cursor_pos {
                            if let Some(idx) = self.hit_row(pos) {
                                self.app.selected = idx;
                            }
                        }
                        let cfg = self.cfg.clone();
                        self.app.handle_action(Action::Confirm, &cfg);
                        if self.app.should_exit {
                            self.exit = true;
                        }
                        self.dirty = true;
                    }
                    BTN_RIGHT => {
                        self.app.should_exit = true;
                        self.exit = true;
                    }
                    _ => {}
                },

                PointerEventKind::Axis { vertical, .. } => {
                    // Prefer discrete (per-click) scroll; accumulate smooth scroll separately.
                    let steps = if vertical.discrete != 0 {
                        self.scroll_accumulator = 0.0;
                        vertical.discrete
                    } else {
                        self.scroll_accumulator += vertical.absolute;
                        let threshold = 15.0_f64;
                        let s = (self.scroll_accumulator / threshold) as i32;
                        self.scroll_accumulator -= s as f64 * threshold;
                        s
                    };

                    if steps != 0 {
                        let action = if steps > 0 { Action::SelectNext } else { Action::SelectPrev };
                        let cfg = self.cfg.clone();
                        for _ in 0..steps.unsigned_abs() {
                            if self.app.handle_action(action, &cfg) {
                                self.dirty = true;
                            }
                        }
                    }
                }

                PointerEventKind::Release { .. } => {}
            }
        }
    }
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
delegate_pointer!(WaylandState);
delegate_shm!(WaylandState);
delegate_registry!(WaylandState);
