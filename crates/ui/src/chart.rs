//! Per-core DPC latency bar chart rendered on a `Canvas`.
//!
//! One bar per logical core, height proportional to the max DPC duration
//! (us) observed in the last 2 s window. Colors follow a fixed threshold
//! scale: green < 100 us (healthy), amber < 250 us (watch), red >= 250 us
//! (audio/glitch territory).

use common::TelemetrySnapshot;
use windows_reactor::{
    Brush, Canvas, CanvasChildExt, Color, KeyedView, LayoutControl, Rectangle, TextBlock, View,
};

const CHART_WIDTH: f64 = 480.0;
const CHART_HEIGHT: f64 = 80.0;
/// DPC duration (us) mapped to full chart height.
const SCALE_US: f32 = 500.0;

fn bar_color(us: f32) -> Color {
    if us < 100.0 {
        Color::rgb(0x4C, 0xAF, 0x50) // green: healthy
    } else if us < 250.0 {
        Color::rgb(0xFF, 0xC1, 0x07) // amber: watch
    } else {
        Color::rgb(0xF4, 0x43, 0x36) // red: glitch territory
    }
}

/// Builds the DPC chart view from the latest telemetry snapshot.
pub fn dpc_chart(snapshot: &TelemetrySnapshot) -> View {
    if snapshot.dpc_max_us.is_empty() {
        return TextBlock::new()
            .text("DPC monitor: waiting for ETW data...")
            .into();
    }

    let cores = snapshot.dpc_max_us.len();
    let bar_width = (CHART_WIDTH / cores as f64).floor().max(2.0);

    // Dynamic child count: `children()` only accepts statically shaped
    // tuples/arrays, so per-core bars go through `keyed_children` (stable
    // per-core keys keep reconciliation cheap at 30 Hz).
    let bars: Vec<KeyedView> = snapshot
        .dpc_max_us
        .iter()
        .enumerate()
        .map(|(core, &us)| {
            let h = ((us / SCALE_US).min(1.0) as f64) * CHART_HEIGHT;
            let bar = Rectangle::new()
                .fill(Brush::from(bar_color(us)))
                .width((bar_width - 1.0).max(1.0))
                .height(h.max(1.0))
                .canvas_left(core as f64 * bar_width)
                .canvas_top(CHART_HEIGHT - h.max(1.0));
            KeyedView::new(core as u32, bar)
        })
        .collect();

    Canvas::new()
        .width(CHART_WIDTH)
        .height(CHART_HEIGHT)
        .keyed_children(bars)
}
