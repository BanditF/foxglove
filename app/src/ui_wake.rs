use std::sync::Arc;

#[derive(Clone)]
pub struct UiWake(Arc<dyn Fn() + Send + Sync>);

impl UiWake {
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn new(wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self(wake)
    }

    /// Wakes the root viewport, whose frame runs the app (and draws Settings inside it).
    /// Foxglove: not `ctx.request_repaint()`, which from another thread asks for whichever
    /// viewport the UI thread is drawing at that moment (egui's shared viewport stack), e.g.
    /// the hidden Settings window's, whose repaints are throttled (settings_open.rs).
    pub fn from_ctx(ctx: &egui::Context) -> Self {
        let ctx = ctx.clone();
        Self(Arc::new(move || {
            ctx.request_repaint_of(egui::ViewportId::ROOT)
        }))
    }

    #[track_caller]
    pub fn request_repaint(&self) {
        crate::foxglove::repaint_trace::wake(std::panic::Location::caller()); // Foxglove: #100
        (self.0)();
    }
}
