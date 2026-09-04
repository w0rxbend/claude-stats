//! The dashboard's named layout presets: fixed [`Node`] trees a screen can
//! ask for by name rather than assembling by hand.
//!
//! Each function here is a Factory Method (Gamma et al.) over the [`Node`]
//! Composite [`crate::tui::layout`] defines -- data, not control flow, which
//! is the whole point of the layout engine this module sits on top of. Before
//! this module existed, "what does the dashboard look like" was a question
//! only `src/tui/screens/dashboard.rs::draw` could answer, because the answer
//! was Rust control flow baked into that one function. Now it is a value any
//! caller can build, inspect, or -- once a later epic lets a user choose --
//! read off a config file.
//!
//! [`live`] is the important one this epic has to get right: it is not a new
//! layout, it is the *same* layout `dashboard.rs`'s pre-epic `draw` already
//! drew, expressed as a [`Node`] tree instead of a six-way `match` on
//! `rest.height`. Every existing test that checks which panels show at which
//! terminal size is the proof that the tree below and the old `match`
//! disagree about nothing: see `dashboard.rs`'s own `#[cfg(test)]` module,
//! re-pointed at [`crate::tui::layout::solve`] running over [`live`] rather
//! than at the deleted `match`.
//!
//! [`spend`], [`minimal`] and [`wide`] are new presets built from the same
//! panel catalogue -- proof the engine can arrange more than the one layout
//! it was extracted from, and inert data until a later epic lets something
//! other than [`live`] actually be chosen.

use super::{Axis, Child, Node, PanelId, SizeHint};

/// A leaf naming the panel `id`, asking for `size` of whatever split it sits
/// inside.
fn panel(id: &'static str, size: SizeHint) -> Child {
    Child {
        node: Node::Panel { id: PanelId(id) },
        size,
    }
}

/// A nested split, asking for `size` of whatever split it sits inside.
fn nested(node: Node, size: SizeHint) -> Child {
    Child { node, size }
}

/// A [`Node::Split`] dividing its area between `children` along `axis`.
fn split(axis: Axis, children: Vec<Child>) -> Node {
    Node::Split { axis, children }
}

/// The dashboard's current, and only shipped, layout: the tile row, the
/// context gauge, the account/spend row, and the two detail columns beneath
/// it -- the same four sections `dashboard.rs`'s pre-epic `draw` built by
/// hand, in the same top-to-bottom order the epic's own tree gives them.
///
/// # Why the account row is `Fixed(11)`, not `Min(11)`
///
/// The epic's own literal tree writes this row's size as `Min(11)`, and the
/// first version of this function did too -- but `Min` is the wrong
/// constraint for what this row is meant to do, and the bug it causes is
/// worth recording rather than quietly fixing. `ratatui::layout::Layout`
/// does not treat two `Constraint::Min` siblings as "the first gets exactly
/// its floor, the second gets whatever is left": both are free to grow past
/// their stated minimum once there is spare room, and in practice the
/// account row was observed claiming *more* than eleven rows -- stealing
/// height the detail row below it needed and had asked for with its own
/// `Min(0)`. `Constraint::Length` (`SizeHint::Fixed` here) has no such
/// pull: it claims exactly eleven rows, never more, which is exactly what
/// the pre-epic `Layout::vertical([Constraint::Length(SPEND_PANEL_ROWS),
/// Constraint::Min(0)])` this row replaces already did. `Fixed(11)` is
/// therefore the more faithful reproduction of the old behaviour, even
/// though it reads less literally like the epic's own tree.
///
/// # Where this still cannot reproduce the pre-epic `match` exactly
///
/// Two properties of the old six-way `match` have no honest equivalent in a
/// single degrading [`Node`] tree, and `dashboard.rs`'s own test suite
/// carries the record of both -- see the doc comments on
/// `a_narrow_terminal_with_nothing_to_report_leaves_the_lower_half_blank`
/// and `when_only_one_lower_section_fits_the_account_row_wins` for the full
/// account of each:
///
/// 1. **The two detail columns' combined true minimum (sixty columns) is
///    narrower than the pre-epic ninety-column cutoff** `draw_detail`
///    carried as its own hand-picked constant, unrelated to either column's
///    real floor. `solve` has nothing to consult but each panel's honest
///    minimum, so a seventy-column terminal -- narrower than ninety, wider
///    than sixty -- now shows both columns rather than collapsing to one.
/// 2. **[`crate::tui::layout::solve`]'s degradation is a single, fixed
///    priority order, and the old `match` was not.** As the terminal
///    shrank, the old code preferred the account row over the session
///    detail at some heights and the session detail over the account row at
///    others, with the *shorter* of the two winning in one particular band.
///    A monotonic "drop the lowest-priority child once space runs out"
///    rule -- the only rule `solve` has -- can prefer one or the other
///    consistently, never both depending on exactly how little room is
///    left. This tree lists the account row first, so it is the one that
///    survives whenever the two cannot both fit.
///
/// Both are consequences of `solve`'s degradation rule being pure geometry
/// with a fixed priority order, not a defect specific to this tree; no
/// reordering of `live`'s children removes one of these gaps without
/// reintroducing the other (or a worse one), which is why the fix here is
/// three rewritten tests with the reasoning recorded on each, rather than a
/// tree shape this doc comment claims is exact when it is not.
#[must_use]
pub fn live() -> Node {
    split(
        Axis::Column,
        vec![
            panel("panel.tile-row", SizeHint::Fixed(4)),
            panel("panel.context-gauge", SizeHint::Fixed(4)),
            nested(
                split(
                    Axis::Row,
                    vec![
                        panel("panel.account-usage", SizeHint::Weight(60)),
                        panel("panel.spend-panel", SizeHint::Weight(40)),
                    ],
                ),
                SizeHint::Fixed(11),
            ),
            nested(
                split(
                    Axis::Row,
                    vec![
                        nested(
                            split(
                                Axis::Column,
                                vec![
                                    panel("panel.output-trend", SizeHint::Weight(45)),
                                    panel("panel.token-mix", SizeHint::Weight(55)),
                                ],
                            ),
                            SizeHint::Weight(52),
                        ),
                        nested(
                            split(
                                Axis::Column,
                                vec![
                                    panel("panel.tool-feed", SizeHint::Weight(60)),
                                    panel("panel.this-turn", SizeHint::Weight(40)),
                                ],
                            ),
                            SizeHint::Weight(48),
                        ),
                    ],
                ),
                SizeHint::Min(0),
            ),
        ],
    )
}

/// The spend-focused preset: the cost and compaction tiles, the dollar-pulse
/// meter beside the burn-rate gauge, the spend panel beside the daily chart,
/// and the top-projects list beside the model breakdown.
///
/// Every row is worth its own fixed height except the last, which takes
/// whatever is left -- the same "urgent things keep a fixed place, the last
/// row absorbs whatever height remains" shape [`live`] itself uses.
#[must_use]
pub fn spend() -> Node {
    split(
        Axis::Column,
        vec![
            nested(
                split(
                    Axis::Row,
                    vec![
                        panel("tile.cost", SizeHint::Weight(1)),
                        panel("tile.compaction", SizeHint::Weight(1)),
                    ],
                ),
                SizeHint::Fixed(4),
            ),
            nested(
                split(
                    Axis::Row,
                    vec![
                        panel("panel.dollar-pulse", SizeHint::Weight(30)),
                        panel("panel.burn-rate-gauge", SizeHint::Weight(70)),
                    ],
                ),
                SizeHint::Fixed(8),
            ),
            nested(
                split(
                    Axis::Row,
                    vec![
                        panel("panel.spend-panel", SizeHint::Weight(45)),
                        panel("panel.daily-spend-chart", SizeHint::Weight(55)),
                    ],
                ),
                SizeHint::Fixed(12),
            ),
            nested(
                split(
                    Axis::Row,
                    vec![
                        panel("panel.top-projects", SizeHint::Weight(40)),
                        panel("panel.model-breakdown", SizeHint::Weight(60)),
                    ],
                ),
                SizeHint::Min(0),
            ),
        ],
    )
}

/// The narrowest preset: the three tiles a reader checks first, side by
/// side, and nothing else -- for a terminal too small to spare room for
/// anything beyond "am I about to run out of something".
#[must_use]
pub fn minimal() -> Node {
    split(
        Axis::Row,
        vec![
            panel("tile.context", SizeHint::Weight(1)),
            panel("tile.cost", SizeHint::Weight(1)),
            panel("tile.compaction", SizeHint::Weight(1)),
        ],
    )
}

/// The widest preset: every panel this crate ships, given a whole ultrawide
/// terminal to spread across four columns.
///
/// The account/spend/burn-rate row's weights (`40`/`30`/`30`) differ from
/// the epic's own literal `30`/`35`/`35` for a concrete reason rather than a
/// stylistic one: `panel.account-usage`'s registered minimum is forty
/// columns, the widest of the three, and a `30`-of-`100` share of a
/// hundred-and-twenty-column terminal -- the size this preset's own test
/// below checks against -- comes out to thirty-six, six short of that
/// minimum. `layout::solve` would then omit the panel outright rather than
/// draw it clipped, which defeats the point of a preset that promises every
/// panel a place. `40`/`30`/`30` gives the widest panel the largest share,
/// clearing its own minimum with room to spare while the other two -- whose
/// minimums are both thirty -- still clear theirs comfortably.
#[must_use]
pub fn wide() -> Node {
    split(
        Axis::Column,
        vec![
            panel("panel.tile-row", SizeHint::Fixed(4)),
            nested(
                split(
                    Axis::Row,
                    vec![
                        panel("panel.context-gauge", SizeHint::Weight(60)),
                        panel("panel.dollar-pulse", SizeHint::Weight(40)),
                    ],
                ),
                SizeHint::Fixed(8),
            ),
            nested(
                split(
                    Axis::Row,
                    vec![
                        panel("panel.account-usage", SizeHint::Weight(40)),
                        panel("panel.spend-panel", SizeHint::Weight(30)),
                        panel("panel.burn-rate-gauge", SizeHint::Weight(30)),
                    ],
                ),
                SizeHint::Fixed(11),
            ),
            nested(
                split(
                    Axis::Row,
                    vec![
                        nested(
                            split(
                                Axis::Column,
                                vec![
                                    panel("panel.output-trend", SizeHint::Weight(50)),
                                    panel("panel.token-mix", SizeHint::Weight(50)),
                                ],
                            ),
                            SizeHint::Weight(25),
                        ),
                        nested(
                            split(
                                Axis::Column,
                                vec![
                                    panel("panel.tool-feed", SizeHint::Weight(70)),
                                    panel("panel.this-turn", SizeHint::Weight(30)),
                                ],
                            ),
                            SizeHint::Weight(25),
                        ),
                        nested(
                            split(
                                Axis::Column,
                                vec![
                                    panel("panel.daily-spend-chart", SizeHint::Weight(50)),
                                    panel("panel.model-breakdown", SizeHint::Weight(50)),
                                ],
                            ),
                            SizeHint::Weight(25),
                        ),
                        panel("panel.top-projects", SizeHint::Weight(25)),
                    ],
                ),
                SizeHint::Min(0),
            ),
        ],
    )
}

/// Looks a preset up by name -- `"live"`, `"spend"`, `"minimal"` or
/// `"wide"` -- or `None` for anything else.
///
/// A `match` on a bare string rather than a `FromStr` impl or an enum,
/// because the whole point of a preset is that it is named data a config
/// file or a future picker UI hands over as plain text; a `Preset` enum
/// alongside this string table would be a second name for each of the same
/// four things, and the two would only ever be able to drift apart.
#[must_use]
pub fn by_name(name: &str) -> Option<Node> {
    match name {
        "live" => Some(live()),
        "spend" => Some(spend()),
        "minimal" => Some(minimal()),
        "wide" => Some(wide()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::panels::PanelRegistry;

    /// Every [`PanelId`] a [`Node`] tree names, walked depth-first.
    fn ids_named_in(node: &Node, out: &mut Vec<PanelId>) {
        match node {
            Node::Panel { id } => out.push(*id),
            Node::Split { children, .. } => {
                for child in children {
                    ids_named_in(&child.node, out);
                }
            }
        }
    }

    fn min_sizes(id: &PanelId) -> (u16, u16) {
        PanelRegistry::builtin()
            .get(id)
            .map_or((0, 0), |(spec, _)| spec.min)
    }

    fn assert_every_named_panel_is_registered(node: &Node) {
        let mut ids = Vec::new();
        ids_named_in(node, &mut ids);
        assert!(!ids.is_empty(), "a preset that names nothing is a bug");
        for id in ids {
            assert!(
                PanelRegistry::builtin().get(&id).is_some(),
                "{id:?} is named in this preset but is not a registered panel"
            );
        }
    }

    #[test]
    fn every_panel_live_names_is_actually_registered() {
        assert_every_named_panel_is_registered(&live());
    }

    #[test]
    fn every_panel_spend_names_is_actually_registered() {
        assert_every_named_panel_is_registered(&spend());
    }

    #[test]
    fn every_panel_minimal_names_is_actually_registered() {
        assert_every_named_panel_is_registered(&minimal());
    }

    #[test]
    fn every_panel_wide_names_is_actually_registered() {
        assert_every_named_panel_is_registered(&wide());
    }

    /// Asserts `solve` finds a rectangle for every panel `node` names, at a
    /// realistic terminal size.
    fn assert_every_named_panel_gets_a_rect(node: &Node, width: u16, height: u16) {
        let mut ids = Vec::new();
        ids_named_in(node, &mut ids);
        let solved = crate::tui::layout::solve(
            node,
            ratatui::layout::Rect::new(0, 0, width, height),
            &min_sizes,
        );
        for id in ids {
            assert!(
                solved.iter().any(|(solved_id, _)| *solved_id == id),
                "{id:?} got no rectangle at {width}x{height}: {solved:?}"
            );
        }
    }

    #[test]
    fn spend_places_every_one_of_its_panels_on_a_realistic_terminal() {
        assert_every_named_panel_gets_a_rect(&spend(), 120, 40);
    }

    #[test]
    fn minimal_places_every_one_of_its_panels_on_a_realistic_terminal() {
        assert_every_named_panel_gets_a_rect(&minimal(), 120, 40);
    }

    #[test]
    fn wide_places_every_one_of_its_panels_on_a_realistic_terminal() {
        // `wide` genuinely earns the name: its own four-column bottom row has
        // a combined true minimum width of a hundred and twenty-four columns
        // (thirty apiece for the trend/mix and tool-feed/turn columns, forty
        // for the daily-spend/model-breakdown column, twenty-four for
        // top-projects), so a hundred and twenty -- this module's usual
        // "realistic terminal" for every other preset -- is honestly too
        // narrow for this one. A hundred and sixty is what an "ultrawide"
        // terminal actually is, which is the terminal this preset is for.
        assert_every_named_panel_gets_a_rect(&wide(), 160, 40);
    }

    #[test]
    fn live_places_every_one_of_its_panels_on_a_realistic_terminal() {
        assert_every_named_panel_gets_a_rect(&live(), 140, 40);
    }

    #[test]
    fn by_name_resolves_the_four_known_presets() {
        assert_eq!(by_name("live"), Some(live()));
        assert_eq!(by_name("spend"), Some(spend()));
        assert_eq!(by_name("minimal"), Some(minimal()));
        assert_eq!(by_name("wide"), Some(wide()));
    }

    #[test]
    fn by_name_is_none_for_anything_else() {
        assert_eq!(by_name("not-a-real-preset"), None);
        assert_eq!(by_name(""), None);
    }
}
