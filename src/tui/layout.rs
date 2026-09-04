//! A layout tree that generalises the nested `Layout::vertical`/`horizontal`
//! calls `src/tui/screens/dashboard.rs` hard-codes today.
//!
//! [`Node`] is a Composite (Gamma et al.): a [`Node::Split`] holds a list of
//! [`Child`]ren, each of which is itself a [`Node`] -- a panel, or another
//! split. That uniformity is the whole point. `dashboard.rs`'s own layout is
//! a tree with exactly this shape already -- a vertical split of header,
//! tiles, gauge and "rest", where "rest" is itself a vertical split whose own
//! second half is a horizontal split of two columns, each a further vertical
//! split -- but it is written as Rust control flow, so the only way to change
//! which panel goes where is to edit `dashboard.rs` and recompile. Building
//! the same shape as data instead is what lets a later epic offer more than
//! one preset, or read one from a config file, without dashboard.rs's `draw`
//! changing at all.
//!
//! [`solve`] is the one function that walks a [`Node`] and turns it into
//! concrete `ratatui` rectangles, folding in the degradation rule that used
//! to live as a handful of `if rest.height >= …` branches in `dashboard.rs`:
//! see [`solve`]'s own doc for the four cases it handles.

use ratatui::layout::{Constraint, Direction, Layout, Rect};

pub mod presets;

/// Which way a [`Node::Split`] divides its area.
///
/// Named for the shape the children end up in on screen -- `Row` lays them
/// out left to right, `Column` stacks them top to bottom -- rather than after
/// `ratatui::layout::Direction`, whose `Horizontal`/`Vertical` naming answers
/// a different question (which way the *split line* runs) and reads
/// backwards next to a six-tile row that is visually horizontal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// Children sit side by side; area is divided by width.
    Row,
    /// Children stack top to bottom; area is divided by height.
    Column,
}

/// How much of a [`Node::Split`]'s area one [`Child`] asks for.
///
/// Mirrors the three `ratatui::layout::Constraint` variants this crate
/// actually uses today (`Length`, `Ratio`, `Min` -- see `dashboard.rs` and
/// every widget it lays out), under names that describe *intent* rather than
/// pixels: a tile wants a `Fixed` number of rows regardless of what else is
/// on screen, the account and spend panels split what is left by `Weight`,
/// and a panel with `Min` wants at least this much and will take more if it
/// is offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizeHint {
    /// Exactly this many cells along the split's axis, regardless of the
    /// other children.
    Fixed(u16),
    /// A share of whatever is left, proportional to every other `Weight` in
    /// the same split. A `Weight(1)` beside a `Weight(2)` gets a third of the
    /// remainder; the second gets two thirds.
    Weight(u16),
    /// At least this many cells, growing to fill any space left over once
    /// every other constraint in the split has been satisfied.
    Min(u16),
}

/// How a [`PanelRegistry`] panel is allowed to grow past its minimum.
///
/// [`PanelRegistry`]: crate::tui::panels::PanelRegistry
///
/// This is metadata a panel carries about itself -- read by whatever chooses
/// a [`SizeHint`] for it, not consulted by [`solve`], which only ever acts on
/// the `SizeHint`s a [`Node`] was actually built with. Keeping the two
/// separate is what lets a panel be dropped into a `Fixed`-sized slot in one
/// preset and a `Weight`-sized one in another without the panel itself
/// changing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flex {
    /// Only worth growing wider; its height is fixed at its minimum.
    Width,
    /// Only worth growing taller; its width is fixed at its minimum.
    Height,
    /// Worth growing in either direction.
    Both,
    /// Grows in fixed steps rather than smoothly -- the context banner is the
    /// one panel like this today, stepping from its four-row half-height
    /// rendering straight to an eight-row full-height one with nothing
    /// useful in between.
    Quantised,
}

/// One child of a [`Node::Split`]: what it is, and how much of the split's
/// area it asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Child {
    pub node: Node,
    pub size: SizeHint,
}

/// A node in the layout tree: either a leaf naming one registered panel, or
/// a split dividing its area between further nodes.
///
/// No `Box` anywhere in sight even though this is a directly recursive type:
/// [`Node::Split`] holds its children in a `Vec`, and a `Vec`'s own size
/// does not depend on the size of what it holds, so the recursion bottoms
/// out through the `Vec`'s heap allocation exactly the way it would through
/// an explicit `Box`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    /// Divides its area between `children`, along `axis`.
    Split { axis: Axis, children: Vec<Child> },
    /// A leaf: draw whatever [`PanelRegistry`] has registered under `id`.
    ///
    /// [`PanelRegistry`]: crate::tui::panels::PanelRegistry
    Panel { id: PanelId },
}

/// The name a panel is registered under, e.g. `PanelId("tile.context")` or
/// `PanelId("panel.spend-panel")`.
///
/// A thin wrapper over `&'static str` rather than a bare string, so that a
/// [`Node`] naming a panel that was never registered is still a compile-time
/// possibility a reviewer can see written down as a distinct type -- and so
/// that `crate::tui::panels::PanelRegistry`'s `HashMap` key is a type that
/// says what it is for, the same reason `crate::domain::project::SessionId`
/// wraps a bare `String` rather than every session-id parameter in this
/// crate reading `id: String`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PanelId(pub &'static str);

/// Walks `node`, turning it into the rectangles each named panel should
/// actually be drawn into, in `area`.
///
/// `min_sizes` answers "how small can this panel be and still be honest",
/// as `(min_width, min_height)`, for any [`PanelId`] the tree names --
/// [`PanelRegistry::get`] is the real answer to that question, and every
/// test in this module supplies a stand-in closure instead so the solver's
/// own rules can be pinned without a registry in hand.
///
/// [`PanelRegistry::get`]: crate::tui::panels::PanelRegistry::get
///
/// # The degradation rule
///
/// A [`Node::Split`]'s own minimum extent, along its own [`Axis`], is the
/// **sum** of its children's minimums on that axis; across the other axis it
/// is their **maximum** -- a row of tiles needs the width of all six added
/// together, but only as much height as the tallest one.
///
/// From that:
///
/// 1. **A [`Node::Panel`] whose own minimum cannot be met, even alone,**
///    produces no rectangle at all. A clipped panel shows a number with its
///    units cut off, which reads as a different, wrong number; showing
///    nothing is the honest alternative `crate::tui::widgets::stat_tile`
///    already chose for exactly this reason, and `solve` applies the same
///    rule to every panel rather than leaving each widget to notice on its
///    own.
/// 2. **A [`Node::Split`] on [`Axis::Column`] whose children collectively do
///    not fit** the height it was given drops the child at the *end* of its
///    `children` list and solves again with what remains, repeating until
///    either the reduced list fits or only one child is left (which then
///    falls under rule 1 if it still does not fit alone). List order is
///    therefore priority order: put what should survive the longest first.
/// 3. **A [`Node::Split`] on [`Axis::Row`] whose children collectively do not
///    fit the width it was given** does not shed children one at a time the
///    way a column does. It collapses in one step to its *first* child alone,
///    given the *entire* area rather than whatever a share of it would have
///    been -- a row of tiles that cannot all fit is more useful as one wide
///    tile than as a partial row of narrow ones. A row's later children are
///    therefore not a priority order the way a column's are: they simply do
///    not survive a row that has run out of width, and only the first
///    child's own placement (`children[0]`) decides what a starved row falls
///    back to.
///
/// Once a split's current children do fit, `ratatui::layout::Layout` divides
/// the area between them from their [`SizeHint`]s -- `Fixed` becomes
/// [`Constraint::Length`], `Weight` becomes [`Constraint::Ratio`] against the
/// sum of every `Weight` in the split, `Min` becomes [`Constraint::Min`] --
/// and `solve` recurses into each child's slice in turn.
#[must_use]
pub fn solve(
    node: &Node,
    area: Rect,
    min_sizes: &dyn Fn(&PanelId) -> (u16, u16),
) -> Vec<(PanelId, Rect)> {
    match node {
        Node::Panel { id } => solve_panel(*id, area, min_sizes),
        Node::Split { axis, children } => solve_split(*axis, children, area, min_sizes),
    }
}

/// Rule 1: a panel below its own minimum is omitted rather than clipped.
fn solve_panel(
    id: PanelId,
    area: Rect,
    min_sizes: &dyn Fn(&PanelId) -> (u16, u16),
) -> Vec<(PanelId, Rect)> {
    let (min_width, min_height) = min_sizes(&id);
    if area.width < min_width || area.height < min_height {
        Vec::new()
    } else {
        vec![(id, area)]
    }
}

/// Rules 2 and 3: degrade `children` until what remains fits `area`, then
/// divide `area` between them and recurse.
fn solve_split(
    axis: Axis,
    children: &[Child],
    area: Rect,
    min_sizes: &dyn Fn(&PanelId) -> (u16, u16),
) -> Vec<(PanelId, Rect)> {
    let Some((first, rest)) = children.split_first() else {
        return Vec::new();
    };

    if fits(axis, children, area, min_sizes) {
        return divide_and_recurse(axis, children, area, min_sizes);
    }

    if axis == Axis::Row {
        // Rule 3: a starved row does not shed children one at a time -- it
        // collapses in one step to its first child, given the whole area.
        return solve(&first.node, area, min_sizes);
    }

    if rest.is_empty() {
        // Down to the one child a column split degrades to, and it still
        // does not fit: `solve_panel`/the recursive `solve` call below
        // already knows how to say "nothing" rather than clip it.
        return solve(&first.node, area, min_sizes);
    }

    // Rule 2: drop the child at the end of the list -- the lowest priority,
    // by the ordering this function's own doc on `solve` asks callers to
    // choose -- and solve again with what remains.
    solve_split(axis, &children[..children.len() - 1], area, min_sizes)
}

/// Whether `children`, laid out along `axis`, fit inside `area` -- both along
/// the split's own axis (their minimums summed) and across it (their
/// minimums' maximum).
fn fits(
    axis: Axis,
    children: &[Child],
    area: Rect,
    min_sizes: &dyn Fn(&PanelId) -> (u16, u16),
) -> bool {
    let (needed_along, needed_across) = extent(axis, children, min_sizes);
    match axis {
        Axis::Row => u32::from(area.width) >= needed_along && area.height >= needed_across,
        Axis::Column => u32::from(area.height) >= needed_along && area.width >= needed_across,
    }
}

/// `children`'s own minimum extent along `axis` (summed) and across it
/// (the maximum), as `(along, across)`.
///
/// `along` is widened to `u32` because it is a sum over however many
/// children a split has; `across` stays `u16` because a maximum can never
/// exceed its widest input.
fn extent(
    axis: Axis,
    children: &[Child],
    min_sizes: &dyn Fn(&PanelId) -> (u16, u16),
) -> (u32, u16) {
    let mins: Vec<(u16, u16)> = children
        .iter()
        .map(|child| node_min(&child.node, min_sizes))
        .collect();
    match axis {
        Axis::Row => (
            mins.iter().map(|(w, _)| u32::from(*w)).sum(),
            mins.iter().map(|(_, h)| *h).max().unwrap_or(0),
        ),
        Axis::Column => (
            mins.iter().map(|(_, h)| u32::from(*h)).sum(),
            mins.iter().map(|(w, _)| *w).max().unwrap_or(0),
        ),
    }
}

/// A node's own minimum size, as `(min_width, min_height)` -- a panel's is
/// whatever `min_sizes` says; a split's is computed from its *full* list of
/// children (dropping only ever happens locally, at the split that failed to
/// fit its parent, never pre-emptively inside a nested subtree).
fn node_min(node: &Node, min_sizes: &dyn Fn(&PanelId) -> (u16, u16)) -> (u16, u16) {
    match node {
        Node::Panel { id } => min_sizes(id),
        Node::Split { axis, children } => {
            let (along, across) = extent(*axis, children, min_sizes);
            let along = u16::try_from(along).unwrap_or(u16::MAX);
            match axis {
                Axis::Row => (along, across),
                Axis::Column => (across, along),
            }
        }
    }
}

/// `children` are already known to fit `area`: divide it between them with
/// `ratatui`'s own constraint solver and recurse into each child's slice.
fn divide_and_recurse(
    axis: Axis,
    children: &[Child],
    area: Rect,
    min_sizes: &dyn Fn(&PanelId) -> (u16, u16),
) -> Vec<(PanelId, Rect)> {
    let total_weight: u16 = children
        .iter()
        .map(|child| match child.size {
            SizeHint::Weight(weight) => weight,
            SizeHint::Fixed(_) | SizeHint::Min(_) => 0,
        })
        .sum();
    let constraints: Vec<Constraint> = children
        .iter()
        .map(|child| to_constraint(child.size, total_weight))
        .collect();
    let direction = match axis {
        Axis::Row => Direction::Horizontal,
        Axis::Column => Direction::Vertical,
    };
    let slices = Layout::new(direction, constraints).split(area);

    children
        .iter()
        .zip(slices.iter())
        .flat_map(|(child, slice)| solve(&child.node, *slice, min_sizes))
        .collect()
}

/// Translates one [`SizeHint`] into the `ratatui::layout::Constraint` it
/// stands for -- see [`solve`]'s doc for why each pairing was chosen.
///
/// `total_weight` of `0` (every sibling is `Fixed` or `Min`, or a lone
/// `Weight(0)`) has no proportion to take a share of; `Ratio(0, 1)` asks for
/// none of the area, which is the honest answer rather than a divide by
/// zero.
fn to_constraint(size: SizeHint, total_weight: u16) -> Constraint {
    match size {
        SizeHint::Fixed(length) => Constraint::Length(length),
        SizeHint::Weight(weight) if total_weight == 0 => {
            debug_assert_eq!(
                weight, 0,
                "a positive weight always contributes to the total"
            );
            Constraint::Ratio(0, 1)
        }
        SizeHint::Weight(weight) => Constraint::Ratio(u32::from(weight), u32::from(total_weight)),
        SizeHint::Min(min) => Constraint::Min(min),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel(id: &'static str, size: SizeHint) -> Child {
        Child {
            node: Node::Panel { id: PanelId(id) },
            size,
        }
    }

    fn area(width: u16, height: u16) -> Rect {
        Rect::new(0, 0, width, height)
    }

    /// Every panel in these tests is `10x4` unless a test says otherwise --
    /// small enough that a handful of them fit an ordinary terminal width,
    /// and distinct enough from `0` that "did not fit" and "was never
    /// measured" cannot be confused.
    fn uniform_minimums(min: (u16, u16)) -> impl Fn(&PanelId) -> (u16, u16) {
        move |_| min
    }

    fn rect_for<'a>(solved: &'a [(PanelId, Rect)], id: &str) -> Option<&'a Rect> {
        solved
            .iter()
            .find(|(panel_id, _)| panel_id.0 == id)
            .map(|(_, rect)| rect)
    }

    #[test]
    fn a_split_that_fits_places_every_child() {
        let node = Node::Split {
            axis: Axis::Row,
            children: vec![
                panel("a", SizeHint::Weight(1)),
                panel("b", SizeHint::Weight(1)),
            ],
        };
        let solved = solve(&node, area(40, 10), &uniform_minimums((10, 4)));

        assert_eq!(solved.len(), 2);
        let a = rect_for(&solved, "a").expect("a fits");
        let b = rect_for(&solved, "b").expect("b fits");
        assert_eq!(a.width, 20, "an even split of a weight-1/weight-1 row");
        assert_eq!(b.width, 20);
    }

    #[test]
    fn a_column_split_drops_its_last_child_when_space_is_short() {
        // Three panels stacked, each needing four rows: nine rows is enough
        // for the first two but not the third.
        let node = Node::Split {
            axis: Axis::Column,
            children: vec![
                panel("first", SizeHint::Fixed(4)),
                panel("second", SizeHint::Fixed(4)),
                panel("third", SizeHint::Fixed(4)),
            ],
        };
        let solved = solve(&node, area(20, 9), &uniform_minimums((10, 4)));

        assert!(rect_for(&solved, "first").is_some());
        assert!(rect_for(&solved, "second").is_some());
        assert!(
            rect_for(&solved, "third").is_none(),
            "the lowest-priority child is the one dropped: {solved:?}"
        );
    }

    #[test]
    fn a_panel_below_its_own_minimum_is_omitted_rather_than_clipped() {
        let node = Node::Panel {
            id: PanelId("tiny"),
        };
        let solved = solve(&node, area(5, 2), &uniform_minimums((10, 4)));

        assert!(
            solved.is_empty(),
            "a 5x2 area cannot honestly show a 10x4 minimum: {solved:?}"
        );
    }

    #[test]
    fn a_panel_that_meets_its_minimum_exactly_is_still_drawn() {
        let node = Node::Panel {
            id: PanelId("exact"),
        };
        let solved = solve(&node, area(10, 4), &uniform_minimums((10, 4)));

        assert_eq!(solved, vec![(PanelId("exact"), area(10, 4))]);
    }

    #[test]
    fn a_row_split_collapses_to_its_first_child_alone_when_too_narrow() {
        // Two panels side by side, each needing ten columns: a twenty-wide
        // row would give each of them an even, and honest, ten-column share,
        // but twelve columns cannot fit both. Rule 3 does not shrink either
        // one -- it collapses to the first child and hands it the *whole*
        // twelve columns, not a half share of them.
        let node = Node::Split {
            axis: Axis::Row,
            children: vec![
                panel("left", SizeHint::Weight(1)),
                panel("right", SizeHint::Weight(1)),
            ],
        };
        let solved = solve(&node, area(12, 4), &uniform_minimums((10, 4)));

        assert_eq!(
            solved,
            vec![(PanelId("left"), area(12, 4))],
            "the first child gets the entire area, not a half share of it, and the second \
             does not survive a starved row at all"
        );
    }

    #[test]
    fn a_starved_row_whose_first_child_still_does_not_fit_alone_renders_nothing() {
        // The same starved row, but even the whole twelve-column area is not
        // enough for the first child's own twenty-column minimum. Rule 3's
        // collapse still hands it the area; rule 1 is then the one that
        // decides nothing is drawn, rather than a clipped tile.
        let node = Node::Split {
            axis: Axis::Row,
            children: vec![
                panel("left", SizeHint::Weight(1)),
                panel("right", SizeHint::Weight(1)),
            ],
        };
        let solved = solve(&node, area(12, 4), &uniform_minimums((20, 4)));

        assert_eq!(solved, vec![], "neither child can be honestly drawn");
    }

    #[test]
    fn nested_splits_recurse_into_each_solved_slice() {
        let node = Node::Split {
            axis: Axis::Column,
            children: vec![Child {
                node: Node::Split {
                    axis: Axis::Row,
                    children: vec![
                        panel("left", SizeHint::Weight(1)),
                        panel("right", SizeHint::Weight(1)),
                    ],
                },
                size: SizeHint::Min(4),
            }],
        };
        let solved = solve(&node, area(20, 4), &uniform_minimums((5, 4)));

        assert_eq!(solved.len(), 2);
        assert_eq!(rect_for(&solved, "left").expect("fits").width, 10);
        assert_eq!(rect_for(&solved, "right").expect("fits").width, 10);
    }

    #[test]
    fn a_splits_own_minimum_sums_along_its_axis_and_maxes_across_it() {
        let node = Node::Split {
            axis: Axis::Row,
            children: vec![
                panel("a", SizeHint::Fixed(10)),
                panel("b", SizeHint::Fixed(10)),
            ],
        };
        // Along the row's own axis (width) the two ten-wide panels need
        // twenty together; across it (height) the split needs only the
        // taller of the two, not both added up.
        let min = node_min(&node, &|id| if id.0 == "a" { (10, 4) } else { (10, 6) });
        assert_eq!(min, (20, 6));
    }

    #[test]
    fn an_empty_split_solves_to_nothing_rather_than_panicking() {
        let node = Node::Split {
            axis: Axis::Row,
            children: vec![],
        };
        assert_eq!(
            solve(&node, area(40, 10), &uniform_minimums((10, 4))),
            vec![]
        );
    }
}
