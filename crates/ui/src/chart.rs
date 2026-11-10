//! Per-core DPC latency bar chart rendered on a `Canvas`.
//!
//! One bar per logical core, height proportional to the max DPC duration
//! (us) observed in the last 2 s window. Colors follow a fixed threshold
//! scale: green < 100 us (healthy), amber < 250 us (watch), red >= 250 us
//! (audio/glitch territory).

use common::{CoreInfo, TelemetrySnapshot};
use windows_reactor::{
    Brush, Canvas, CanvasChildExt, ChildrenControl, Color, KeyedView, LayoutControl, Rectangle,
    TextBlock, View,
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

const CELL: f64 = 22.0;
const CELL_GAP: f64 = 3.0;
const CELLS_PER_ROW: usize = 16;

/// P/E core map: one cell per logical core, blue = P-core, green = E-core.
pub fn cpu_map(cores: &[CoreInfo]) -> View {
    if cores.is_empty() {
        return TextBlock::new().text("CPU topology: unavailable").into();
    }

    let cells: Vec<KeyedView> = cores
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let col = (i % CELLS_PER_ROW) as f64;
            let row = (i / CELLS_PER_ROW) as f64;
            // efficiency_class: 0 = E-core, 1 = P-core.
            let color = if c.efficiency_class == 1 {
                Color::rgb(0x21, 0x96, 0xF3) // blue: P-core
            } else {
                Color::rgb(0x81, 0xC7, 0x84) // green: E-core
            };
            let cell = Rectangle::new()
                .fill(Brush::from(color))
                .width(CELL)
                .height(CELL)
                .canvas_left(col * (CELL + CELL_GAP))
                .canvas_top(row * (CELL + CELL_GAP));
            KeyedView::new(i as u32, cell)
        })
        .collect();

    let rows = (cores.len() + CELLS_PER_ROW - 1) / CELLS_PER_ROW;
    Canvas::new()
        .width(CELLS_PER_ROW as f64 * (CELL + CELL_GAP))
        .height(rows as f64 * (CELL + CELL_GAP))
        .keyed_children(cells)
}
