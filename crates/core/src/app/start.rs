// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Options in the page's URL (or, on desktop, on the command line) that set
//! the app up at start, so that a link opens on a known state: a sample as
//! the reference, the viewpoints, the study aids, the panels, the tool.
//!
//! [`StartOptions::parse`] reads a query string without touching the app,
//! so every shell parses the same way; [`App::apply_url_options`] applies
//! it once. None of it is a change to the clay: the timelapse never sees
//! it.
//!
//! - `sample=<id>` loads a bundled sample as the reference.
//! - `view=<names>` sets both views; `sculpt=<names>` and
//!   `reference=<names>` set one. Names, separated by commas:
//!   - a viewpoint: `front`, `back`, `left`, `right`, `top`, `bottom`,
//!     `three-quarter`;
//!   - an aid each view has of its own: `silhouette`, `mirror`, `plumb`,
//!     `grid`, `ortho` (or `perspective`), and a lighting: `studio`,
//!     `raking`, `top-light`;
//!   - an aid both views share: `ghost`, `wireframe`, `difference`,
//!     `link`, `swap`, `measure`;
//!   - in `view=` only, the views that show: `both`, `sculpt`,
//!     `reference`.
//!
//!   `no-<name>` turns a switch off (`no-link`, `no-silhouette`).
//! - `grid=<n>`: the grid's divisions, 2, 3, 4, 6 or 8.
//! - `hide=panels` hides the side panels (Tab); `hide=reference` or
//!   `hide=sculpt` shows the other view alone; `hide=panels,reference` is
//!   Shift+Tab, the sculpt alone without panels.
//! - `tool=<name>`: `move`, `clay`, `flatten`, `smooth`, `inflate`,
//!   `crease`, `pinch`, `mask`.
//! - `symmetry`, `symmetry=on` or `symmetry=off`.
//! - `timer=<minutes>` or `timer=<minutes>x<rounds>` starts the pose timer
//!   (minutes 1, 2, 5, 10, 20 or 30; rounds 1, 3, 5 or 10).
//!
//! Later options win over earlier ones; the link wins over a viewpoint
//! given to the reference view alone, which then follows the sculpt.
//! Unknown keys and names are logged and skipped. `lang`, `debug`,
//! `timing` and `fresh` are the shells' own.

use super::help::query_pairs;
use super::study::{GRID_DIVISIONS, Layout, TIMER_MINUTES, TIMER_ROUNDS};
use super::views::{VIEWPOINT_IDS, viewpoints};
use super::{App, REFERENCE, SCULPT};
use crate::reference::samples::Sample;
use crate::render::clay::Lighting;
use crate::sculpt::Brush;

/// One view's own settings; `None` where the URL leaves them be.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct ViewSettings {
    /// An index into `viewpoints`.
    viewpoint: Option<usize>,
    silhouette: Option<bool>,
    mirror: Option<bool>,
    plumb: Option<bool>,
    grid: Option<bool>,
    ortho: Option<bool>,
    lighting: Option<Lighting>,
}

/// What both views share.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Shared {
    ghost: Option<bool>,
    wireframe: Option<bool>,
    difference: Option<bool>,
    link: Option<bool>,
    swap: Option<bool>,
    measure: Option<bool>,
}

/// The state a query string asks the app to start in. See the module's
/// documentation for the options.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StartOptions {
    sample: Option<&'static str>,
    /// Indexed by `SCULPT` and `REFERENCE`.
    views: [ViewSettings; 2],
    shared: Shared,
    layout: Option<Layout>,
    hide_panels: bool,
    grid_divisions: Option<u32>,
    tool: Option<Brush>,
    symmetry: Option<bool>,
    /// Minutes and rounds.
    timer: Option<(u32, u32)>,
    /// Any option besides the sample, understood or not.
    sets_state: bool,
    /// What wasn't understood, as written, for the log.
    unknown: Vec<String>,
}

/// The tools' names in `tool=`.
fn tool_id(brush: Brush) -> &'static str {
    match brush {
        Brush::Move => "move",
        Brush::Clay => "clay",
        Brush::Flatten => "flatten",
        Brush::Smooth => "smooth",
        Brush::Inflate => "inflate",
        Brush::Crease => "crease",
        Brush::Pinch => "pinch",
        Brush::Mask => "mask",
        // Held with keys, never chosen: `tool=` looks in `Brush::ALL` only.
        Brush::BlurMask => "blur-mask",
    }
}

/// The lightings' names in a view's list.
fn lighting_id(lighting: Lighting) -> &'static str {
    match lighting {
        Lighting::Studio => "studio",
        Lighting::Raking => "raking",
        Lighting::Top => "top-light",
    }
}

/// `on` for a bare key.
fn on_off(value: &str) -> Option<bool> {
    match value {
        "" | "on" | "1" | "true" | "yes" => Some(true),
        "off" | "0" | "false" | "no" => Some(false),
        _ => None,
    }
}

impl ViewSettings {
    /// Takes one name of a view's list; false if it isn't one of a view's
    /// own.
    fn set(&mut self, name: &str, on: bool) -> bool {
        let switch = match name {
            "silhouette" => &mut self.silhouette,
            "mirror" => &mut self.mirror,
            "plumb" => &mut self.plumb,
            "grid" => &mut self.grid,
            "ortho" => &mut self.ortho,
            "perspective" => {
                self.ortho = Some(!on);
                return true;
            }
            _ if !on => return false,
            _ => {
                if let Some(k) = VIEWPOINT_IDS.iter().position(|&id| id == name) {
                    self.viewpoint = Some(k);
                } else if let Some(&l) = Lighting::ALL.iter().find(|&&l| lighting_id(l) == name) {
                    self.lighting = Some(l);
                } else {
                    return false;
                }
                return true;
            }
        };
        *switch = Some(on);
        true
    }
}

impl Shared {
    /// Takes one name of a view's list; false if it isn't a shared aid.
    fn set(&mut self, name: &str, on: bool) -> bool {
        let switch = match name {
            "ghost" => &mut self.ghost,
            "wireframe" => &mut self.wireframe,
            "difference" => &mut self.difference,
            "link" => &mut self.link,
            "swap" => &mut self.swap,
            "measure" => &mut self.measure,
            _ => return false,
        };
        *switch = Some(on);
        true
    }
}

impl StartOptions {
    /// Reads a query string (`?a=1&b=2`, with or without the `?`). Never
    /// fails: what it doesn't know is kept aside for the log.
    pub fn parse(query: &str) -> Self {
        let mut options = Self::default();
        for (key, value) in query_pairs(query) {
            // Some tools escape the commas.
            let value = value.replace("%2C", ",").replace("%2c", ",");
            let value = value.to_ascii_lowercase();
            let state = match key {
                "sample" => {
                    match Sample::by_id(&value) {
                        Some(sample) => options.sample = Some(sample.id),
                        None => options.unknown.push(format!("sample={value}")),
                    }
                    false
                }
                "view" => {
                    options.names(&value, [SCULPT, REFERENCE].as_slice(), true);
                    true
                }
                "sculpt" => {
                    options.names(&value, &[SCULPT], false);
                    true
                }
                "reference" => {
                    options.names(&value, &[REFERENCE], false);
                    true
                }
                "grid" => {
                    match value.parse().ok().filter(|n| GRID_DIVISIONS.contains(n)) {
                        Some(n) => options.grid_divisions = Some(n),
                        None => options.unknown.push(format!("grid={value}")),
                    }
                    true
                }
                "hide" => {
                    for name in list(&value) {
                        match name {
                            "panels" => options.hide_panels = true,
                            "reference" => options.layout = Some(Layout::Sculpt),
                            "sculpt" => options.layout = Some(Layout::Reference),
                            _ => options.unknown.push(format!("hide={name}")),
                        }
                    }
                    true
                }
                "tool" => {
                    match Brush::ALL.into_iter().find(|&b| tool_id(b) == value) {
                        Some(brush) => options.tool = Some(brush),
                        None => options.unknown.push(format!("tool={value}")),
                    }
                    true
                }
                "symmetry" => {
                    match on_off(&value) {
                        Some(on) => options.symmetry = Some(on),
                        None => options.unknown.push(format!("symmetry={value}")),
                    }
                    true
                }
                "timer" => {
                    match parse_timer(&value) {
                        Some(timer) => options.timer = Some(timer),
                        None => options.unknown.push(format!("timer={value}")),
                    }
                    true
                }
                "lang" | "debug" | "timing" | "fresh" => false,
                _ => {
                    options.unknown.push(key.to_owned());
                    false
                }
            };
            options.sets_state |= state;
        }
        options
    }

    /// Whether the options set up the app beyond the sample: then the
    /// last session shouldn't come back over them.
    pub fn sets_state(&self) -> bool {
        self.sets_state
    }

    /// One list of names for the views `indices`; with `layout`, the
    /// views that show can be chosen too.
    fn names(&mut self, value: &str, indices: &[usize], layout: bool) {
        for name in list(value) {
            let (on, bare) = match name.strip_prefix("no-") {
                Some(bare) => (false, bare),
                None => (true, name),
            };
            let chosen = match bare {
                "both" if layout && on => Some(Layout::Both),
                "sculpt" if layout && on => Some(Layout::Sculpt),
                "reference" if layout && on => Some(Layout::Reference),
                _ => None,
            };
            if chosen.is_some() {
                self.layout = chosen;
                continue;
            }
            let mut known = self.shared.set(bare, on);
            if !known {
                for &index in indices {
                    known = self.views[index].set(bare, on);
                }
            }
            if !known {
                self.unknown.push(name.to_owned());
            }
        }
    }
}

/// The names in a comma-separated list, trimmed, without empty ones.
fn list(value: &str) -> impl Iterator<Item = &str> {
    value.split(',').map(str::trim).filter(|n| !n.is_empty())
}

/// `5`, `5x3`, or a bare `timer` for the menu's first choice: minutes and
/// rounds, among those the menu offers.
fn parse_timer(value: &str) -> Option<(u32, u32)> {
    if value.is_empty() {
        return Some((5, 1));
    }
    let (minutes, rounds) = value.split_once('x').unwrap_or((value, "1"));
    let minutes = minutes.parse().ok().filter(|m| TIMER_MINUTES.contains(m))?;
    let rounds = rounds.parse().ok().filter(|r| TIMER_ROUNDS.contains(r))?;
    Some((minutes, rounds))
}

impl App {
    /// Applies the options in the page's query string or, on desktop, on
    /// the command line, joined by `&`: see [`StartOptions`]. The sample
    /// is fetched as the gallery does, showing it loading; the rest applies
    /// now.
    pub fn apply_url_options(&mut self, query: &str) {
        self.apply_start_options(&StartOptions::parse(query));
    }

    fn apply_start_options(&mut self, o: &StartOptions) {
        for unknown in &o.unknown {
            log::warn!("URL option not understood: {unknown}");
        }
        if let Some(sample) = o.sample.and_then(Sample::by_id) {
            self.load_sample(sample);
        }
        let st = &mut self.study;
        if let Some(layout) = o.layout {
            st.layout = layout;
            self.panels.settle(layout);
        }
        if let Some(n) = o.grid_divisions {
            st.grid_divisions = n;
        }
        let set = |to: &mut bool, from: Option<bool>| {
            if let Some(on) = from {
                *to = on;
            }
        };
        for index in [SCULPT, REFERENCE] {
            let (from, aids) = (&o.views[index], &mut st.views[index]);
            set(&mut aids.silhouette, from.silhouette);
            set(&mut aids.mirror, from.mirror);
            set(&mut aids.plumb, from.plumb);
            set(&mut aids.grid, from.grid);
            if let Some(lighting) = from.lighting {
                aids.lighting = lighting;
            }
        }
        let shared = &o.shared;
        set(&mut st.ghost, shared.ghost);
        set(&mut st.wireframe, shared.wireframe);
        set(&mut st.matching.tint, shared.difference);
        set(&mut st.swapped, shared.swap);
        set(&mut st.link, shared.link);
        if let Some(on) = shared.measure {
            st.measure.on = on;
            st.measure.lines = [None; 2];
            st.measure.drawing = None;
        }
        let points = viewpoints(self.lang.tr());
        for index in [SCULPT, REFERENCE] {
            let (from, camera) = (&o.views[index], &mut self.cameras[index]);
            set(&mut camera.ortho, from.ortho);
            if let Some(k) = from.viewpoint {
                let (_, _, yaw, pitch) = points[k];
                camera.look_from(yaw, pitch);
            }
        }
        // Linked, the reference looks as the sculpt does.
        self.follow_camera(SCULPT);
        if o.hide_panels {
            if self.study.layout == Layout::Sculpt {
                // Shift+Tab, as if pressed with both views showing: Tab
                // brings them back with the panels.
                if !self.panels.focused() {
                    self.study.layout = Layout::Both;
                    self.panels
                        .toggle_focus(&mut self.study.layout, Layout::Sculpt);
                }
            } else if !self.panels.hidden() {
                self.toggle_panels();
            }
        }
        if let Some(tool) = o.tool {
            self.tool.tool = tool;
        }
        if let Some(on) = o.symmetry {
            self.tool.symmetry = on;
        }
        if let Some((minutes, rounds)) = o.timer {
            let timer = &mut self.study.timer;
            timer.minutes = minutes;
            timer.rounds = rounds;
            timer.start(self.now);
        }
        self.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::super::study::tests::Harness;
    use super::*;

    #[test]
    fn names_set_both_views_or_one() {
        let o = StartOptions::parse(
            "?view=three-quarter,silhouette&reference=no-silhouette,mirror,top-light",
        );
        assert!(o.sets_state());
        assert_eq!(o.views[SCULPT].viewpoint, Some(6));
        assert_eq!(o.views[REFERENCE].viewpoint, Some(6));
        assert_eq!(o.views[SCULPT].silhouette, Some(true));
        assert_eq!(o.views[REFERENCE].silhouette, Some(false));
        assert_eq!(o.views[REFERENCE].mirror, Some(true));
        assert_eq!(o.views[SCULPT].mirror, None);
        assert_eq!(o.views[REFERENCE].lighting, Some(Lighting::Top));
        assert!(o.unknown.is_empty(), "{:?}", o.unknown);
    }

    #[test]
    fn every_viewpoint_lighting_and_tool_has_a_name() {
        for (k, id) in VIEWPOINT_IDS.iter().enumerate() {
            let o = StartOptions::parse(&format!("sculpt={id}"));
            assert_eq!(o.views[SCULPT].viewpoint, Some(k));
            assert_eq!(o.views[REFERENCE].viewpoint, None);
        }
        let tr = crate::Lang::En.tr();
        assert_eq!(viewpoints(tr).len(), VIEWPOINT_IDS.len());
        for lighting in Lighting::ALL {
            let o = StartOptions::parse(&format!("view={}", lighting_id(lighting)));
            assert_eq!(o.views[SCULPT].lighting, Some(lighting));
        }
        for brush in Brush::ALL {
            let o = StartOptions::parse(&format!("tool={}", tool_id(brush)));
            assert_eq!(o.tool, Some(brush));
        }
    }

    #[test]
    fn shared_aids_layout_and_the_rest() {
        let o = StartOptions::parse(
            "sample=brutus&view=Link,ghost,no-swap,reference&sculpt=wireframe,perspective\
             &grid=3&hide=panels&tool=Smooth&symmetry&timer=10x3",
        );
        assert_eq!(o.sample, Some("brutus"));
        assert_eq!(o.shared.link, Some(true));
        assert_eq!(o.shared.ghost, Some(true));
        assert_eq!(o.shared.swap, Some(false));
        assert_eq!(o.shared.wireframe, Some(true));
        assert_eq!(o.views[SCULPT].ortho, Some(false));
        assert_eq!(o.layout, Some(Layout::Reference));
        assert_eq!(o.grid_divisions, Some(3));
        assert!(o.hide_panels);
        assert_eq!(o.tool, Some(Brush::Smooth));
        assert_eq!(o.symmetry, Some(true));
        assert_eq!(o.timer, Some((10, 3)));
        assert!(o.unknown.is_empty(), "{:?}", o.unknown);
    }

    #[test]
    fn unknown_values_are_skipped_not_fatal() {
        let o = StartOptions::parse(
            "view=sideways,grid&sculpt=both&grid=5&tool=chisel&timer=7&hide=menu&symmetry=maybe&zoom=2",
        );
        assert_eq!(o.views[SCULPT].grid, Some(true));
        assert_eq!(o.layout, None, "layouts are view= only");
        assert_eq!(o.grid_divisions, None);
        assert_eq!(o.tool, None);
        assert_eq!(o.timer, None);
        assert_eq!(o.symmetry, None);
        assert_eq!(
            o.unknown,
            [
                "sideways",
                "both",
                "grid=5",
                "tool=chisel",
                "timer=7",
                "hide=menu",
                "symmetry=maybe",
                "zoom"
            ]
        );
    }

    #[test]
    fn only_the_shells_options_and_a_sample_leave_the_session_alone() {
        assert!(!StartOptions::parse("?lang=ja&debug&fresh&sample=venus").sets_state());
        assert!(!StartOptions::parse("").sets_state());
        assert!(StartOptions::parse("tool=clay").sets_state());
        // Even misspelt, a demo option means a set-up start.
        assert!(StartOptions::parse("view=silhouete").sets_state());
        let o = StartOptions::parse("view=mirror%2Cplumb");
        assert_eq!(o.views[REFERENCE].plumb, Some(true));
    }

    #[test]
    fn the_app_starts_as_the_url_says() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        h.app.apply_url_options(
            "view=left,silhouette,link&reference=raking,ortho&grid=6&hide=panels&tool=flatten\
             &symmetry=on&timer=2",
        );
        h.frames(2);
        let app = &h.app;
        let st = &app.study;
        assert!(st.views[SCULPT].silhouette && st.views[REFERENCE].silhouette);
        assert_eq!(st.views[REFERENCE].lighting, Lighting::Raking);
        assert_eq!(st.views[SCULPT].lighting, Lighting::Studio);
        assert_eq!(st.grid_divisions, 6);
        assert!(st.link);
        // Linked: the reference follows the sculpt, projection and all.
        assert_eq!(app.cameras[REFERENCE], app.cameras[SCULPT]);
        let mut left = crate::camera::OrbitCamera::default();
        left.look_from(-std::f32::consts::FRAC_PI_2, 0.0);
        assert_eq!(app.cameras[SCULPT].yaw, left.yaw);
        assert!(app.panels.hidden() && !app.panels.focused());
        assert_eq!(app.tool.tool, Brush::Flatten);
        assert!(app.tool.symmetry);
        assert_eq!(st.timer.minutes, 2);
        assert_ne!(st.timer.clock, super::super::study::Clock::Off);
        assert!(!app.sculpt.can_undo(), "nothing in the clay's record");
    }

    #[test]
    fn hiding_the_panels_and_the_reference_is_shift_tab() {
        let Some(mut h) = Harness::new() else {
            eprintln!("no GPU adapter; skipping app test");
            return;
        };
        h.app.apply_url_options("hide=panels,reference");
        h.frames(1);
        assert!(h.app.panels.focused() && h.app.panels.hidden());
        assert_eq!(h.app.study.layout, Layout::Sculpt);
        h.app.toggle_panels();
        assert_eq!(h.app.study.layout, Layout::Both);
        assert!(!h.app.panels.hidden());
    }
}
