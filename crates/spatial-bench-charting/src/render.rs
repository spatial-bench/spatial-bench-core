//! Rendering: run-document series → an RGB buffer via plotters, then either
//! a PNG or the kitty graphics protocol.
//!
//! Division of labour: plotters draws the mesh (axes, ticks, decade labels on
//! the log scale — the fiddly part); this module maps data coordinates to
//! pixels through the chart and draws the data itself (bars, lines, CI
//! whiskers, labels, legend) as pixel-space elements. The buffer is rendered
//! to memory so the same bytes go to the terminal or a file.

use crate::model::Series;
use plotters::coord::combinators::IntoLogRange;
use plotters::coord::Shift;
use plotters::prelude::*;
use std::path::Path;

/// A mapped series: the colour it will be drawn in and every point in
/// plotting-area pixels.
pub struct MappedSeries {
    pub name: String,
    pub color: RGBColor,
    pub points: Vec<MappedPoint>,
}

#[derive(Debug, Clone, Copy)]
pub struct MappedPoint {
    pub x: i32,
    pub y: i32,
    pub lower: Option<i32>,
    pub upper: Option<i32>,
    /// The data-space y value, for labels (pixels cannot be inverted).
    pub value: f64,
}

struct Palette {
    bg: RGBColor,
    fg: RGBColor,
    grid: RGBColor,
    series: Vec<RGBColor>,
}

fn palette(dark: bool) -> Palette {
    let series = vec![
        RGBColor(0x42, 0x84, 0xF4),
        RGBColor(0xF4, 0x6A, 0x42),
        RGBColor(0x2C, 0xA5, 0x5C),
        RGBColor(0x9B, 0x51, 0xE0),
        RGBColor(0xE6, 0xB8, 0x1E),
        RGBColor(0x18, 0xA9, 0x9B),
    ];
    if dark {
        Palette {
            bg: RGBColor(0x1E, 0x1E, 0x24),
            fg: RGBColor(0xE8, 0xE8, 0xE8),
            grid: RGBColor(0x3A, 0x3A, 0x44),
            series,
        }
    } else {
        Palette {
            bg: WHITE,
            fg: BLACK,
            grid: RGBColor(0xC0, 0xC0, 0xC8),
            series: series
                .iter()
                .map(|c| RGBColor(c.0 / 2 + 32, c.1 / 2 + 32, c.2 / 2 + 32))
                .collect(),
        }
    }
}

/// A text style in the embedded font, coloured. `FontDesc::new` with the
/// registered family name is how the ab_glyph backend resolves fonts —
/// hermetic: no reliance on system fonts (CI containers often ship none).
fn ts(size: i32, color: &RGBColor) -> TextStyle<'static> {
    TextStyle {
        font: FontDesc::new(
            FontFamily::from("sans-serif"),
            size as f64,
            FontStyle::Normal,
        ),
        color: color.to_backend_color(),
        pos: plotters::style::text_anchor::Pos::default(),
    }
}

/// The chart title, from the selection and mode.
pub fn title_for(
    metric: &str,
    series_tag: &str,
    x_tag: Option<&str>,
    runner: &str,
    selection: &str,
) -> String {
    let kind = if x_tag.is_some() {
        "scaling"
    } else {
        "comparison"
    };
    format!("{metric} by {series_tag} — {kind} ({runner})\n{selection}")
}

/// Human-readable metric values for labels: 70 → "70", 16742 → "16.7k",
/// 1.66e8 → "166M".
pub fn fmt_value(v: f64) -> String {
    let a = v.abs();
    if a >= 1e9 {
        format!("{:.1}G", v / 1e9)
    } else if a >= 1e6 {
        format!("{:.1}M", v / 1e6)
    } else if a >= 1e3 {
        format!("{:.1}k", v / 1e3)
    } else {
        format!("{v:.0}")
    }
}

/// Render one chart into an RGB buffer (w × h × 3 bytes, row-major) —
/// plotters' BitMapBackend native format, and what the kitty protocol's
/// `f=24` expects.
///
/// `bars` mode compares implementations at one x category (bar height =
/// metric value, whiskers = CI); `lines` mode plots scaling across the x
/// axis. Log y-scale is the default because the subjects differ by orders of
/// magnitude — kiddo ~70ns vs pykdtree ~16.7µs is a flat line on a linear
/// axis.
/// The chart's non-data configuration.
pub struct ChartSpec {
    pub title: String,
    pub y_desc: String,
    pub x_desc: String,
    pub bars: bool,
    pub log_y: bool,
    pub width: u32,
    pub height: u32,
    pub dark: bool,
}

pub fn render(spec: &ChartSpec, series: &[Series]) -> Result<Vec<u8>, String> {
    let mut buf = vec![0u8; (spec.width * spec.height * 4) as usize];
    {
        let backend = BitMapBackend::with_buffer(&mut buf, (spec.width, spec.height));
        let root = backend.into_drawing_area();
        draw_chart(&root, series, spec)?;
        root.present().map_err(|e| format!("present: {e}"))?;
    }
    Ok(buf)
}

/// Save a chart as a PNG by rendering straight into a file-backed backend
/// (real compression via the image encoder; PR-comment uploads should not be
/// megabytes).
pub fn render_to_png(spec: &ChartSpec, series: &[Series], path: &Path) -> Result<(), String> {
    let backend = BitMapBackend::new(path, (spec.width, spec.height));
    let root = backend.into_drawing_area();
    draw_chart(&root, series, spec)?;
    root.present()
        .map_err(|e| format!("writing {}: {e}", path.display()))
}

fn draw_chart(
    root: &DrawingArea<BitMapBackend, Shift>,
    series: &[Series],
    spec: &ChartSpec,
) -> Result<(), String> {
    let title = &spec.title;
    let y_desc = &spec.y_desc;
    let x_desc = &spec.x_desc;
    let bars = spec.bars;
    let log_y = spec.log_y;
    let dark = spec.dark;
    let height = spec.height;
    let pal = palette(dark);

    // Hermetic typography: an embedded font, no reliance on system fonts.
    // Re-registering overwrites the entry, so calls are idempotent.
    // The y range must cover the data and its error bars, floored at a small
    // positive value for the log scale.
    let mut y_min = f64::INFINITY;
    let mut y_max = f64::NEG_INFINITY;
    for s in series {
        for &(_, y, lo, hi) in &s.points {
            y_min = y_min.min(lo.unwrap_or(y));
            y_max = y_max.max(hi.unwrap_or(y));
        }
    }
    if !y_min.is_finite() || !y_max.is_finite() || y_min <= 0.0 {
        return Err(
            "the data contains non-positive or non-finite values; only positive \
             latency-like metrics chart today"
                .to_owned(),
        );
    }
    let (y_lo, y_hi) = if log_y {
        let span_decades = (y_max / y_min).log10().ceil();
        (
            y_min * 0.8,
            y_max * 10f64.powf(span_decades * 0.5).max(1.05),
        )
    } else {
        let span = (y_max - y_min).max(1.0);
        ((y_min - span * 0.1).max(0.0), y_max + span * 0.1)
    };

    let title_size = (height as f64 * 0.045) as i32;
    let label_size = ((height as f64 * 0.028) as i32).max(10);

    root.fill(&pal.bg).map_err(|e| format!("fill: {e}"))?;

    let map_series = |chart: &ChartContext<
        BitMapBackend,
        Cartesian2d<plotters::coord::types::RangedCoordf64, plotters::coord::types::RangedCoordf64>,
    >| {
        series
            .iter()
            .enumerate()
            .map(|(i, s)| MappedSeries {
                name: s.name.clone(),
                color: pal.series[i % pal.series.len()],
                points: s
                    .points
                    .iter()
                    .map(|&(x, y, lo, hi)| {
                        let cx = if bars { i as f64 + 0.5 } else { x };
                        let at = |v: f64| chart.plotting_area().map_coordinate(&(cx, v));
                        let (px, py) = at(y);
                        MappedPoint {
                            x: px,
                            y: py,
                            lower: lo.map(|v| at(v).1),
                            upper: hi.map(|v| at(v).1),
                            value: y,
                        }
                    })
                    .collect(),
            })
            .collect::<Vec<MappedSeries>>()
    };
    let map_series_log = |chart: &ChartContext<
        BitMapBackend,
        Cartesian2d<
            plotters::coord::types::RangedCoordf64,
            plotters::coord::combinators::LogCoord<f64>,
        >,
    >| {
        series
            .iter()
            .enumerate()
            .map(|(i, s)| MappedSeries {
                name: s.name.clone(),
                color: pal.series[i % pal.series.len()],
                points: s
                    .points
                    .iter()
                    .map(|&(x, y, lo, hi)| {
                        let cx = if bars { i as f64 + 0.5 } else { x };
                        let at = |v: f64| chart.plotting_area().map_coordinate(&(cx, v));
                        let (px, py) = at(y);
                        MappedPoint {
                            x: px,
                            y: py,
                            lower: lo.map(|v| at(v).1),
                            upper: hi.map(|v| at(v).1),
                            value: y,
                        }
                    })
                    .collect(),
            })
            .collect::<Vec<MappedSeries>>()
    };

    if bars {
        let x_max = series.len() as f64;
        if log_y {
            let mut chart = ChartBuilder::on(root)
                .caption(title, ("sans-serif", title_size, &pal.fg))
                .margin(14)
                .x_label_area_size(44)
                .y_label_area_size(72)
                .build_cartesian_2d(0f64..x_max, (y_lo..y_hi).log_scale())
                .map_err(|e| format!("building the chart: {e}"))?;
            chart
                .configure_mesh()
                .disable_x_mesh()
                .x_labels(0)
                .y_desc(y_desc)
                .x_desc(x_desc)
                .label_style(("sans-serif", label_size, &pal.fg))
                .axis_style(pal.fg)
                .light_line_style(pal.grid)
                .draw()
                .map_err(|e| format!("drawing the mesh: {e}"))?;
            let mapped = map_series_log(&chart);
            let floor_y = chart.plotting_area().map_coordinate(&(0.5, y_lo)).1;
            drop(chart);
            draw_bars(root, &mapped, &pal, floor_y, label_size, title_size);
        } else {
            let mut chart = ChartBuilder::on(root)
                .caption(title, ("sans-serif", title_size, &pal.fg))
                .margin(14)
                .x_label_area_size(44)
                .y_label_area_size(72)
                .build_cartesian_2d(0f64..x_max, y_lo..y_hi)
                .map_err(|e| format!("building the chart: {e}"))?;
            chart
                .configure_mesh()
                .disable_x_mesh()
                .x_labels(0)
                .y_desc(y_desc)
                .x_desc(x_desc)
                .label_style(("sans-serif", label_size, &pal.fg))
                .axis_style(pal.fg)
                .light_line_style(pal.grid)
                .draw()
                .map_err(|e| format!("drawing the mesh: {e}"))?;
            let mapped = map_series(&chart);
            let floor_y = chart.plotting_area().map_coordinate(&(0.5, y_lo)).1;
            drop(chart);
            draw_bars(root, &mapped, &pal, floor_y, label_size, title_size);
        }
    } else {
        let x_min = series
            .iter()
            .flat_map(|s| s.points.iter().map(|p| p.0))
            .fold(f64::INFINITY, f64::min);
        let x_max = series
            .iter()
            .flat_map(|s| s.points.iter().map(|p| p.0))
            .fold(f64::NEG_INFINITY, f64::max);
        let (x_lo, x_hi) = (x_min * 0.9, x_max * 1.1);
        if log_y {
            let mut chart = ChartBuilder::on(root)
                .caption(title, ("sans-serif", title_size, &pal.fg))
                .margin(14)
                .x_label_area_size(44)
                .y_label_area_size(72)
                .build_cartesian_2d(x_lo..x_hi, (y_lo..y_hi).log_scale())
                .map_err(|e| format!("building the chart: {e}"))?;
            chart
                .configure_mesh()
                .y_desc(y_desc)
                .x_desc(x_desc)
                .label_style(("sans-serif", label_size, &pal.fg))
                .axis_style(pal.fg)
                .light_line_style(pal.grid)
                .draw()
                .map_err(|e| format!("drawing the mesh: {e}"))?;
            let mapped = map_series_log(&chart);
            drop(chart);
            draw_lines(root, &mapped, &pal, label_size, title_size);
        } else {
            let mut chart = ChartBuilder::on(root)
                .caption(title, ("sans-serif", title_size, &pal.fg))
                .margin(14)
                .x_label_area_size(44)
                .y_label_area_size(72)
                .build_cartesian_2d(x_lo..x_hi, y_lo..y_hi)
                .map_err(|e| format!("building the chart: {e}"))?;
            chart
                .configure_mesh()
                .y_desc(y_desc)
                .x_desc(x_desc)
                .label_style(("sans-serif", label_size, &pal.fg))
                .axis_style(pal.fg)
                .light_line_style(pal.grid)
                .draw()
                .map_err(|e| format!("drawing the mesh: {e}"))?;
            let mapped = map_series(&chart);
            drop(chart);
            draw_lines(root, &mapped, &pal, label_size, title_size);
        }
    }

    root.present().map_err(|e| format!("present: {e}"))?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn draw_bars<B: DrawingBackend>(
    root: &DrawingArea<B, Shift>,
    mapped: &[MappedSeries],
    pal: &Palette,
    floor_y: i32,
    label_size: i32,
    title_size: i32,
) {
    // Bar half-width from the pixel distance between the first two bars, or
    // a single-bar default.
    let half = if mapped.len() >= 2 && !mapped[0].points.is_empty() && !mapped[1].points.is_empty()
    {
        ((mapped[1].points[0].x - mapped[0].points[0].x).abs() / 3).clamp(8, 120)
    } else {
        40
    };
    for s in mapped {
        for p in &s.points {
            let rect = Rectangle::new(
                [(p.x - half, floor_y), (p.x + half, p.y)],
                s.color.mix(0.3).filled().stroke_width(2),
            );
            root.draw(&rect).ok();
            if let (Some(lo), Some(hi)) = (p.lower, p.upper) {
                root.draw(&ErrorBar::new_vertical(
                    p.x,
                    lo.min(hi),
                    p.y,
                    hi.max(lo),
                    s.color.filled(),
                    5,
                ))
                .ok();
            }
            root.draw(&Text::new(
                fmt_value(p.value),
                (p.x, p.y - label_size - 4),
                ts(label_size, &pal.fg),
            ))
            .ok();
            root.draw(&Text::new(
                s.name.clone(),
                (p.x, floor_y + label_size / 2 + 4),
                ts(label_size, &pal.fg),
            ))
            .ok();
        }
    }
    draw_legend(root, mapped, pal, label_size, title_size);
}

#[allow(clippy::too_many_arguments)]
fn draw_lines<B: DrawingBackend>(
    root: &DrawingArea<B, Shift>,
    mapped: &[MappedSeries],
    pal: &Palette,
    label_size: i32,
    title_size: i32,
) {
    for s in mapped {
        let path: Vec<(i32, i32)> = s.points.iter().map(|p| (p.x, p.y)).collect();
        root.draw(&PathElement::new(path, s.color.stroke_width(2)))
            .ok();
        for p in &s.points {
            root.draw(&Circle::new((p.x, p.y), 3, s.color.filled()))
                .ok();
            if let (Some(lo), Some(hi)) = (p.lower, p.upper) {
                root.draw(&ErrorBar::new_vertical(
                    p.x,
                    lo.min(hi),
                    p.y,
                    hi.max(lo),
                    s.color.filled(),
                    4,
                ))
                .ok();
            }
        }
    }
    draw_legend(root, mapped, pal, label_size, title_size);

    draw_legend(root, mapped, pal, label_size, title_size);
}

fn draw_legend<B: DrawingBackend>(
    root: &DrawingArea<B, Shift>,
    mapped: &[MappedSeries],
    pal: &Palette,
    label_size: i32,
    title_size: i32,
) {
    // The legend: top-right box with colour swatches and series names.
    let line_h = label_size + 8;
    let box_w = mapped.iter().map(|s| s.name.len()).max().unwrap_or(8) as i32 * (label_size / 2)
        + line_h * 2;
    let box_h = mapped.len() as i32 * line_h + 12;
    let (root_w, _) = root.get_pixel_range();
    let box_x = root_w.end - box_w - 8;
    let box_y = title_size + 12;
    root.draw(&Rectangle::new(
        [(box_x, box_y), (box_x + box_w, box_y + box_h)],
        pal.bg.mix(0.85).filled().stroke_width(1),
    ))
    .ok();
    for (i, s) in mapped.iter().enumerate() {
        let y = box_y + 8 + i as i32 * line_h + line_h / 2;
        let swatch = Rectangle::new(
            [(box_x + 8, y - 5), (box_x + 8 + 14, y + 5)],
            s.color.filled(),
        );
        root.draw(&swatch).ok();
        root.draw(&Text::new(
            s.name.clone(),
            (box_x + 8 + 20, y - label_size / 2),
            ts(label_size, &pal.fg),
        ))
        .ok();
    }
}
