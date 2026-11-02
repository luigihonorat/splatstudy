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
}

impl Layout {
    pub const ALL: [Layout; 3] = [Layout::SideBySide, Layout::Stacked, Layout::SculptOnly];

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

/// The views and the 1px rule between them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    pub sculpt: PxRect,
    pub reference: Option<PxRect>,
    pub divider: Option<PxRect>,
}

/// Splits a `size` frame for `layout`. The sculpt gets the first half,
/// rounded down; the rule is one pixel; the reference gets the rest.
pub fn place(size: [u32; 2], layout: Layout) -> Placement {
    let [w, h] = size;
    match layout {
        Layout::SculptOnly => Placement {
            sculpt: PxRect { x: 0, y: 0, w, h },
            reference: None,
            divider: None,
        },
        Layout::SideBySide => {
            let half = w / 2;
            Placement {
                sculpt: PxRect {
                    x: 0,
                    y: 0,
                    w: half,
                    h,
                },
                divider: Some(PxRect {
                    x: half,
                    y: 0,
                    w: 1,
                    h,
                }),
                reference: Some(PxRect {
                    x: half + 1,
                    y: 0,
                    w: w - half - 1,
                    h,
                }),
            }
        }
        Layout::Stacked => {
            let half = h / 2;
            Placement {
                sculpt: PxRect {
                    x: 0,
                    y: 0,
                    w,
                    h: half,
                },
                divider: Some(PxRect {
                    x: 0,
                    y: half,
                    w,
                    h: 1,
                }),
                reference: Some(PxRect {
                    x: 0,
                    y: half + 1,
                    w,
                    h: h - half - 1,
                }),
            }
        }
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
                for layout in Layout::ALL {
                    let p = place(size, layout);
                    let rects: Vec<PxRect> = [Some(p.sculpt), p.divider, p.reference]
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
            PxRect {
                x: 0,
                y: 0,
                w: 960,
                h: 1080
            }
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
        assert_eq!(p.sculpt.size(), [1080, 960]);
        assert_eq!(p.reference.unwrap().y, 961);
    }
}
