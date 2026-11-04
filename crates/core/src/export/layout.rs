// SPDX-FileCopyrightText: 2026 The SplatStudy contributors
// SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
//! Where the views go in an exported frame, in whole pixels.

/// How the two views share the frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Layout {
    /// Sculpt on the left, reference on the right, as in the app.
    #[default]
    SideBySide,
    /// Sculpt above, reference below: for tall frames.
    Stacked,
    SculptOnly,
    /// The reference alone, as when the app shows only its view.
    ReferenceOnly,
}

impl Layout {
    pub const ALL: [Layout; 4] = [
        Layout::SideBySide,
        Layout::Stacked,
        Layout::SculptOnly,
        Layout::ReferenceOnly,
    ];

    pub fn shows_sculpt(self) -> bool {
        self != Layout::ReferenceOnly
    }

    pub fn shows_reference(self) -> bool {
        self != Layout::SculptOnly
    }
}

/// The frame's proportions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Shape {
    /// 16:9, for video sites.
    #[default]
    Wide,
    Square,
    /// 9:16, for phone screens.
    Tall,
}

impl Shape {
    pub const ALL: [Shape; 3] = [Shape::Wide, Shape::Square, Shape::Tall];

    /// The frame size with its short side `short` pixels long (1080 for
    /// full HD, less for the image fallback). Always even, as video needs.
    pub fn size(self, short: u32) -> [u32; 2] {
        let short = short & !1;
        let long = (short * 16 / 9) & !1;
        match self {
            Shape::Wide => [long, short],
            Shape::Square => [short, short],
            Shape::Tall => [short, long],
        }
    }

    /// The layout that suits the shape best.
    pub fn natural_layout(self) -> Layout {
        match self {
            Shape::Tall => Layout::Stacked,
            _ => Layout::SideBySide,
        }
    }
}

/// A rectangle in pixels, origin at the top left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PxRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl PxRect {
    pub fn size(self) -> [u32; 2] {
        [self.w, self.h]
    }

    pub fn area(self) -> u64 {
        u64::from(self.w) * u64::from(self.h)
    }
}

/// The views and the 1px rule between them. A view the layout leaves out
/// has no rectangle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    pub sculpt: Option<PxRect>,
    pub reference: Option<PxRect>,
    pub divider: Option<PxRect>,
}

/// Splits a `size` frame for `layout`. The sculpt gets the first half,
/// rounded down; the rule is one pixel; the reference gets the rest.
pub fn place(size: [u32; 2], layout: Layout) -> Placement {
    place_split(size, layout, 0.5, false)
}

/// Splits a `size` frame for `layout` as the app splits its window: the
/// sculpt gets `share` of the width (or height, stacked), rounded down,
/// wherever it sits; with `swapped` the reference comes first, on the
/// left or on top. The rule is one pixel and the other view gets the rest.
pub fn place_split(size: [u32; 2], layout: Layout, share: f32, swapped: bool) -> Placement {
    let [w, h] = size;
    let whole = PxRect { x: 0, y: 0, w, h };
    // The first view's length along `long`, leaving each view a pixel.
    let first = |long: u32| {
        let share = if swapped { 1.0 - share } else { share };
        let cut = (long as f32 * share.clamp(0.0, 1.0)).floor() as u32;
        cut.clamp(1, long.saturating_sub(2).max(1))
    };
    let (a, divider, b) = match layout {
        Layout::SculptOnly => {
            return Placement {
                sculpt: Some(whole),
                reference: None,
                divider: None,
            };
        }
        Layout::ReferenceOnly => {
            return Placement {
                sculpt: None,
                reference: Some(whole),
                divider: None,
            };
        }
        Layout::SideBySide => {
            let cut = first(w);
            (
                PxRect {
                    x: 0,
                    y: 0,
                    w: cut,
                    h,
                },
                PxRect {
                    x: cut,
                    y: 0,
                    w: 1,
                    h,
                },
                PxRect {
                    x: cut + 1,
                    y: 0,
                    w: w - cut - 1,
                    h,
                },
            )
        }
        Layout::Stacked => {
            let cut = first(h);
            (
                PxRect {
                    x: 0,
                    y: 0,
                    w,
                    h: cut,
                },
                PxRect {
                    x: 0,
                    y: cut,
                    w,
                    h: 1,
                },
                PxRect {
                    x: 0,
                    y: cut + 1,
                    w,
                    h: h - cut - 1,
                },
            )
        }
    };
    let (sculpt, reference) = if swapped { (b, a) } else { (a, b) };
    Placement {
        sculpt: Some(sculpt),
        reference: Some(reference),
        divider: Some(divider),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_full_hd_and_even() {
        assert_eq!(Shape::Wide.size(1080), [1920, 1080]);
        assert_eq!(Shape::Square.size(1080), [1080, 1080]);
        assert_eq!(Shape::Tall.size(1080), [1080, 1920]);
        assert_eq!(Shape::Wide.size(720), [1280, 720]);
        for shape in Shape::ALL {
            for short in [1080, 720, 361] {
                let [w, h] = shape.size(short);
                assert!(w % 2 == 0 && h % 2 == 0, "{shape:?} {short}");
            }
        }
    }

    #[test]
    fn views_and_rule_tile_every_frame_without_overlap() {
        for shape in Shape::ALL {
            for short in [1080, 720] {
                let size = shape.size(short);
                for (layout, share, swapped) in Layout::ALL.into_iter().flat_map(|layout| {
                    [
                        (0.5, false),
                        (0.3, false),
                        (0.3, true),
                        (0.8, true),
                        (0.0, false),
                    ]
                    .map(|(share, swapped)| (layout, share, swapped))
                }) {
                    let p = place_split(size, layout, share, swapped);
                    let rects: Vec<PxRect> = [p.sculpt, p.divider, p.reference]
                        .into_iter()
                        .flatten()
                        .collect();
                    // Inside the frame.
                    for r in &rects {
                        assert!(r.w > 0 && r.h > 0);
                        assert!(r.x + r.w <= size[0] && r.y + r.h <= size[1], "{r:?}");
                    }
                    // No overlap, and together they cover the frame.
                    for (i, a) in rects.iter().enumerate() {
                        for b in &rects[i + 1..] {
                            let apart = a.x + a.w <= b.x
                                || b.x + b.w <= a.x
                                || a.y + a.h <= b.y
                                || b.y + b.h <= a.y;
                            assert!(apart, "{a:?} overlaps {b:?}");
                        }
                    }
                    let covered: u64 = rects.iter().map(|r| r.area()).sum();
                    assert_eq!(covered, u64::from(size[0]) * u64::from(size[1]));
                    assert_eq!(p.reference.is_some(), layout.shows_reference());
                    assert_eq!(p.sculpt.is_some(), layout.shows_sculpt());
                    if let Some(d) = p.divider {
                        assert!(d.w == 1 || d.h == 1, "the rule is one pixel");
                    }
                }
            }
        }
    }

    #[test]
    fn full_hd_side_by_side_splits_in_the_middle() {
        let p = place([1920, 1080], Layout::SideBySide);
        assert_eq!(
            p.sculpt,
            Some(PxRect {
                x: 0,
                y: 0,
                w: 960,
                h: 1080
            })
        );
        assert_eq!(
            p.divider,
            Some(PxRect {
                x: 960,
                y: 0,
                w: 1,
                h: 1080
            })
        );
        assert_eq!(
            p.reference,
            Some(PxRect {
                x: 961,
                y: 0,
                w: 959,
                h: 1080
            })
        );
        let p = place([1080, 1920], Layout::Stacked);
        assert_eq!(p.sculpt.unwrap().size(), [1080, 960]);
        assert_eq!(p.reference.unwrap().y, 961);
    }

    #[test]
    fn a_split_keeps_the_apps_shares_and_order() {
        // The sculpt has 30% of the width, on the right when swapped.
        let p = place_split([1000, 500], Layout::SideBySide, 0.3, false);
        assert_eq!(p.sculpt.unwrap().w, 300);
        assert_eq!(p.reference.unwrap().x, 301);
        let p = place_split([1000, 500], Layout::SideBySide, 0.3, true);
        let (sculpt, reference) = (p.sculpt.unwrap(), p.reference.unwrap());
        assert_eq!((reference.x, reference.w), (0, 700));
        assert_eq!(p.divider.unwrap().x, 700);
        assert_eq!((sculpt.x, sculpt.w), (701, 299));
        // Stacked and swapped: the reference on top.
        let p = place_split([1080, 1920], Layout::Stacked, 0.5, true);
        assert_eq!(p.reference.unwrap().y, 0);
        assert_eq!(p.sculpt.unwrap().y, 961);
        // One view alone fills the frame.
        let p = place_split([800, 600], Layout::ReferenceOnly, 0.3, true);
        assert_eq!(p.sculpt, None);
        assert_eq!(p.reference.unwrap().size(), [800, 600]);
        assert_eq!(p.divider, None);
    }
}
