//! The price sheet a run is costed against, where it came from, and which of
//! the three ways of arriving at a cost was used.
//!
//! The catalogue in [`super::model`] is a compile-time table: excellent for
//! knowing what Opus 5 charges, useless for answering the question a report
//! has to answer, which is *whose* prices produced this figure. Anthropic
//! changes a rate between releases, a user corrects one locally, and two runs
//! a fortnight apart then print different totals for the same week with
//! nothing on screen to explain why. A [`PriceSheet`] is the catalogue turned
//! into a value: it can be built, overlaid, handed to a function and, crucially,
//! asked where it came from.
//!
//! [`CostMode`] is the second half of the same story. A cost is either
//! computed from the counters or taken from a figure the source stated, and a
//! report that does not say which it did is a report whose numbers cannot be
//! compared with anyone else's.

use std::fmt;

use super::entry::Entry;
use super::model::{ModelCatalog, ModelId, ModelPricing};
use super::money::Usd;

/// One model's entry in a price sheet.
///
/// The owned twin of [`super::model::CatalogRow`]. The catalogue's row borrows
/// `&'static str` because it is baked into the binary; a sheet's row cannot,
/// because a row can come from a file the user wrote this morning.
#[derive(Debug, Clone, PartialEq)]
pub struct PriceRow {
    /// Matched as a *substring* of a transcript's model string, which is what
    /// makes `anthropic.claude-opus-5` and `claude-opus-5[1m]` resolve to the
    /// same row.
    pub id: String,
    /// Short name for a header or a table, e.g. `"Opus 5"`.
    pub display: String,
    /// Context window in tokens.
    pub context_window: u64,
    /// The five published rates for this model.
    pub pricing: ModelPricing,
}

/// Where a sheet's rates came from.
///
/// Carried on the sheet rather than worked out by whoever prints the report,
/// because by the time a figure reaches a renderer the file it came from is
/// long out of scope. A total is only comparable with another total if both
/// can say which rates produced them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Provenance {
    /// The rates compiled into this release, and nothing else.
    Builtin,
    /// The compiled-in rates with a user's file laid over them.
    Overridden {
        /// Where that file lives, so a surprising figure can be traced to it.
        source: String,
    },
}

impl fmt::Display for Provenance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Builtin => f.write_str("built-in price sheet"),
            Self::Overridden { source } => write!(f, "overridden from {source}"),
        }
    }
}

/// The rates a run is costed against.
///
/// A Registry in Fowler's sense -- a well-known object other code asks for
/// facts about globally shared reference data -- but deliberately an *instance*
/// rather than a static. [`ModelCatalog`] is the static version and stays
/// exactly as it is; this exists because a static cannot be handed a different
/// answer for one run, and cannot be asked where its answer came from. Both of
/// those are things a report needs: one so a user can correct a rate that
/// changed between releases, the other so the footer can say which sheet
/// produced the number above it.
///
/// **Order is load-bearing.** A row's `id` matches as a substring of the
/// transcript's model string and the first hit wins, so `claude-fable-5-1`
/// must be reached before `claude-fable-5` or a 5.1 model is charged the 5.0
/// generation's cache-read rate, which is four times dearer. Every operation
/// on a sheet preserves relative order for that reason, and
/// [`Self::builtin`] preserves the catalogue's declaration order exactly.
#[derive(Debug, Clone, PartialEq)]
pub struct PriceSheet {
    rows: Vec<PriceRow>,
    provenance: Provenance,
}

impl PriceSheet {
    /// What a model no row matches is charged.
    ///
    /// The catalogue's own fallback rather than a second one of this module's
    /// invention, so that a model nobody has catalogued yet costs the same
    /// whichever lookup asks. See [`ModelCatalog::FALLBACK_PRICING`] for why
    /// it is a mid-range guess and emphatically not zero.
    pub const FALLBACK_PRICING: ModelPricing = ModelCatalog::FALLBACK_PRICING;

    /// The rates compiled into this release.
    ///
    /// Built from [`ModelCatalog::entries`] in the catalogue's own declaration
    /// order, so the sheet answers every lookup exactly as the static table
    /// does. That is the reason working without a network is the *default*
    /// here rather than a degraded mode: the compiled-in sheet is the
    /// reviewed, in-tree, correct one, and there is nothing for a fetch to
    /// improve on between releases.
    #[must_use]
    pub fn builtin() -> Self {
        Self {
            rows: ModelCatalog::entries()
                .map(|row| PriceRow {
                    id: row.id.to_owned(),
                    display: row.display.to_owned(),
                    context_window: row.context_window,
                    pricing: row.pricing,
                })
                .collect(),
            provenance: Provenance::Builtin,
        }
    }

    /// A sheet of exactly these rows, in exactly this order.
    ///
    /// The constructor an adapter uses to turn a user's file into something
    /// [`Self::overlaid_with`] can take. Order is the caller's responsibility
    /// and is preserved verbatim, because only the caller knows whether the
    /// rows it read were meant to shadow one another.
    #[must_use]
    pub fn from_rows(rows: Vec<PriceRow>, provenance: Provenance) -> Self {
        Self { rows, provenance }
    }

    /// The first row whose id appears anywhere in `model`.
    ///
    /// Substring rather than equality, matching the catalogue's rule, because
    /// a transcript's model string is not a catalogue key: deployments prefix
    /// it (`anthropic.claude-opus-5`), the API suffixes it with a snapshot date
    /// (`claude-haiku-4-5-20251001`), and Claude Code brackets a context
    /// variant onto it (`claude-opus-5[1m]`). All three name the same model and
    /// all three must be charged the same way.
    fn row_for(&self, model: &ModelId) -> Option<&PriceRow> {
        self.rows
            .iter()
            .find(|row| model.as_str().contains(row.id.as_str()))
    }

    /// The rates for `model`, or `None` when no row matches.
    ///
    /// `None` rather than the fallback so that a caller who wants to *say
    /// something* about an unpriced model -- a warning on stderr, a marker in a
    /// table -- can tell the two cases apart. A caller who only wants a number
    /// should use [`Self::pricing_or_fallback`], which cannot return nothing.
    #[must_use]
    pub fn pricing_for(&self, model: &ModelId) -> Option<ModelPricing> {
        self.row_for(model).map(|row| row.pricing)
    }

    /// The rates to charge `model` at, falling back rather than to nothing.
    ///
    /// The single place the "an unknown model is not free" rule is applied.
    /// Spelling `pricing_for(..).unwrap_or(FALLBACK_PRICING)` at each call site
    /// would work until one call site spelled it `unwrap_or_default` instead,
    /// at which point a newly released model would silently vanish from every
    /// total that went through that site and nothing would look wrong.
    #[must_use]
    pub fn pricing_or_fallback(&self, model: &ModelId) -> ModelPricing {
        self.pricing_for(model).unwrap_or(Self::FALLBACK_PRICING)
    }

    /// The context window for `model`, in tokens.
    ///
    /// Falls back to 200k for an unknown model, which is the conservative
    /// choice: every Claude model has at least that much, so the context bar
    /// reads pessimistically rather than promising head-room that is not there.
    #[must_use]
    pub fn context_window_for(&self, model: &ModelId) -> u64 {
        self.row_for(model)
            .map_or(ModelCatalog::DEFAULT_CONTEXT_WINDOW, |row| {
                row.context_window
            })
    }

    /// A short human-facing name for `model`.
    ///
    /// An unmatched model gets the catalogue's own fallback name -- the raw
    /// string with `claude-` trimmed off -- rather than the word "unknown", so
    /// a model that shipped this morning is still recognisable in a header.
    #[must_use]
    pub fn display_name_for(&self, model: &ModelId) -> String {
        self.row_for(model).map_or_else(
            || ModelCatalog::fallback_display_name(model.as_str()),
            |row| row.display.clone(),
        )
    }

    /// Every row, in matching order.
    ///
    /// Exists so that `claude-stats models` prints *the* catalogue rather than
    /// a hand-maintained list beside it. The command used to name its own
    /// seven models, which meant the table went stale the moment a row was
    /// added: on the day this was written it showed six of the fourteen models
    /// the crate actually prices, and nothing about the output said so.
    pub fn rows(&self) -> impl Iterator<Item = &PriceRow> {
        self.rows.iter()
    }

    /// This sheet with `other`'s rows laid over it.
    ///
    /// Where an id appears in both, `other`'s row replaces this one's *in
    /// place*. Keeping the position is the whole subtlety: appending an
    /// overriding `claude-fable-5` to the end would leave it sitting after
    /// `claude-fable-5-1`, and since the first substring hit wins, the 5.1
    /// model would go on being priced by whatever row it reached first. An
    /// override is meant to change one rate, not to reshuffle the table.
    ///
    /// A row whose id this sheet has never seen is inserted immediately before
    /// the first row that would otherwise have swallowed it -- the first row
    /// whose id is contained in the new one. Without that, an override adding
    /// `claude-opus-5-turbo` would sit behind `claude-opus-5`, match nothing,
    /// and leave the user staring at a file that visibly has no effect. A new
    /// row that nothing shadows goes at the end, where its position cannot
    /// matter.
    ///
    /// The result carries `other`'s provenance, because the rates a report
    /// printed were composed with `other`'s file and a footer that said
    /// otherwise would be the one thing this type exists to prevent.
    #[must_use]
    pub fn overlaid_with(mut self, other: Self) -> Self {
        for row in other.rows {
            if let Some(index) = self.rows.iter().position(|existing| existing.id == row.id) {
                self.rows[index] = row;
                continue;
            }
            match self
                .rows
                .iter()
                .position(|existing| row.id.contains(existing.id.as_str()))
            {
                Some(shadowing) => self.rows.insert(shadowing, row),
                None => self.rows.push(row),
            }
        }
        self.provenance = other.provenance;
        self
    }

    /// Where these rates came from, for the report footer.
    #[must_use]
    pub const fn provenance(&self) -> &Provenance {
        &self.provenance
    }
}

/// How a caller wants a cost arrived at.
///
/// Three ways of answering "what did this response cost", kept as an explicit
/// choice rather than a hardcoded rule because the three answers are not
/// interchangeable and a report that does not say which one it used cannot be
/// compared with anything.
///
/// # What this format actually records today
///
/// No assistant entry in a Claude Code transcript carries a per-message cost.
/// The transcript records token counters and never a dollar figure, so
/// [`Entry::recorded_cost`] is `None` for every entry this crate reads. The
/// consequence is worth stating plainly rather than leaving a reader to work
/// out from the code: today [`Self::Auto`] and [`Self::Calculate`] agree
/// everywhere, and [`Self::Display`] reports nothing at all.
///
/// The three arms exist anyway for two reasons. The flag is part of the
/// surface users of tools in this space expect, so it has to be answerable;
/// and a future Claude Code that does record costs -- or an imported billing
/// export, which states them outright -- must be a change to the *data*, not a
/// new enum and a new set of call sites.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum CostMode {
    /// Trust a stated cost where there is one, price the tokens where there is
    /// not.
    ///
    /// The default because it is the only mode that is right whatever the
    /// source turns out to record: it never contradicts a figure the source
    /// stated, and never reports nothing merely because the source was quiet.
    #[default]
    Auto,
    /// Always price the tokens, ignoring any stated figure.
    ///
    /// The mode to use when comparing two runs. It is the only one whose
    /// answer does not depend on which entries happened to carry a cost, so
    /// two totals produced this way differ only if the traffic differed --
    /// which is the question a comparison is asking.
    Calculate,
    /// Report only what was stated, and nothing where nothing was.
    ///
    /// Honest about the gap rather than filling it. Against a source that
    /// states no costs this reports [`Usd::ZERO`] throughout, and that is the
    /// point: it shows how much of the bill the source is actually willing to
    /// vouch for.
    Display,
}

impl CostMode {
    /// What `entry` cost, under this mode, at `sheet`'s rates.
    ///
    /// An entry whose model matches no row is charged the sheet's fallback
    /// rather than nothing -- see [`PriceSheet::pricing_or_fallback`]. Nothing
    /// is printed about it here: a warning belongs on stderr where it cannot
    /// corrupt a report being piped into something else, and this function is
    /// called once per response over hundreds of thousands of responses.
    #[must_use]
    pub fn cost_of(self, entry: &Entry, sheet: &PriceSheet) -> Usd {
        match self {
            Self::Auto => entry
                .recorded_cost
                .unwrap_or_else(|| entry.cost(sheet.pricing_or_fallback(&entry.model))),
            Self::Calculate => entry.cost(sheet.pricing_or_fallback(&entry.model)),
            Self::Display => entry.recorded_cost.unwrap_or(Usd::ZERO),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::entry::EntryId;
    use crate::domain::money::RatePerMillionTokens;
    use crate::domain::project::{Project, SessionId};
    use crate::domain::tokens::TokenUsage;

    fn model(id: &str) -> ModelId {
        ModelId::new(id)
    }

    /// One response on `model_id`, carrying `tokens` and whatever cost the
    /// source stated.
    fn entry(model_id: &str, tokens: TokenUsage, recorded: Option<Usd>) -> Entry {
        Entry {
            id: EntryId {
                message_id: "msg_01".to_owned(),
                request_id: Some("req_01".to_owned()),
                session: SessionId::new("session-a"),
            },
            at: "2026-09-01T12:00:00Z".parse().expect("a valid timestamp"),
            model: ModelId::new(model_id),
            tokens,
            recorded_cost: recorded,
            session: SessionId::new("session-a"),
            project: Project::new("/home/ada/api"),
            is_sidechain: false,
        }
    }

    /// A million of each of the five kinds of token, so that a cost in dollars
    /// reads as the sum of the published per-million rates with no arithmetic
    /// in between.
    const ONE_MILLION_OF_EACH: TokenUsage = TokenUsage {
        input: 1_000_000,
        cache_read: 1_000_000,
        cache_write_5m: 1_000_000,
        cache_write_1h: 1_000_000,
        output: 1_000_000,
    };

    fn row(id: &str, display: &str, pricing: ModelPricing) -> PriceRow {
        PriceRow {
            id: id.to_owned(),
            display: display.to_owned(),
            context_window: 1_000_000,
            pricing,
        }
    }

    #[test]
    fn the_builtin_sheet_knows_every_model_the_catalogue_knows() {
        // The sheet is built from the catalogue, so the two must agree row for
        // row and, more importantly, in order: the order is the matching rule,
        // and a sheet that held the same rows shuffled would price the 5.1
        // models at the 5.0 generation's rates while looking complete.
        let sheet = PriceSheet::builtin();
        let catalogue: Vec<_> = ModelCatalog::entries().collect();
        let rows: Vec<_> = sheet.rows().collect();

        assert_eq!(rows.len(), catalogue.len(), "one row per catalogue entry");
        for (row, entry) in rows.iter().zip(&catalogue) {
            assert_eq!(row.id, entry.id, "in the catalogue's own order");
            assert_eq!(row.display, entry.display);
            assert_eq!(row.context_window, entry.context_window);
            assert_eq!(row.pricing, entry.pricing);
            assert_eq!(
                sheet.pricing_for(&model(entry.id)),
                Some(entry.pricing),
                "{} must still resolve to its own row",
                entry.id
            );
        }
        assert_eq!(sheet.provenance(), &Provenance::Builtin);
    }

    #[test]
    fn a_point_release_is_not_swallowed_by_the_release_it_follows() {
        // Fable 5.1 reads cost a fortieth of its input rate; Fable 5 reads cost
        // a tenth. If the 5.1 row is reached second, every 5.1 cache read is
        // charged four times over -- and since real traffic is overwhelmingly
        // cache reads, that is most of the bill rather than a rounding detail.
        let sheet = PriceSheet::builtin();
        let point_release = sheet
            .pricing_for(&model("claude-fable-5-1"))
            .expect("a catalogued model");

        assert!(
            (point_release.cache_read.dollars_per_million() - 0.25).abs() < 1e-9,
            "$0.25/MTok, not the $1.00/MTok its predecessor charges"
        );
        assert_eq!(
            sheet.display_name_for(&model("claude-fable-5-1")),
            "Fable 5.1"
        );

        let predecessor = sheet
            .pricing_for(&model("claude-fable-5"))
            .expect("a catalogued model");
        assert!((predecessor.cache_read.dollars_per_million() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn a_dated_snapshot_id_resolves_to_its_family() {
        // The API appends the snapshot date to the model it actually served.
        // Nobody types that suffix and no table can list every one of them, so
        // it has to fall out of the substring rule.
        let sheet = PriceSheet::builtin();
        let dated = model("claude-haiku-4-5-20251001");

        assert_eq!(
            sheet.pricing_for(&dated),
            sheet.pricing_for(&model("claude-haiku-4-5"))
        );
        assert_eq!(sheet.display_name_for(&dated), "Haiku 4.5");
        assert_eq!(sheet.context_window_for(&dated), 200_000);
    }

    #[test]
    fn a_bracketed_context_variant_resolves_to_its_base_model() {
        // Claude Code writes the enlarged-context variant as `[1m]` on the end
        // of the model string. It is the same model at the same rates.
        let sheet = PriceSheet::builtin();
        let bracketed = model("claude-opus-5[1m]");

        assert_eq!(
            sheet.pricing_for(&bracketed),
            sheet.pricing_for(&model("claude-opus-5"))
        );
        assert_eq!(sheet.display_name_for(&bracketed), "Opus 5");

        // And a vendor prefix, which arrives on the other end of the string.
        assert_eq!(
            sheet.pricing_for(&model("anthropic.claude-opus-5")),
            sheet.pricing_for(&model("claude-opus-5"))
        );
    }

    #[test]
    fn an_unknown_model_is_priced_at_the_fallback_rather_than_free() {
        // A model that ships before the catalogue catches up is still being
        // charged for. Pricing it at nothing would subtract a whole model from
        // every total on the machine of whoever adopts it first, and the report
        // would look entirely healthy while doing it.
        let sheet = PriceSheet::builtin();
        let unheard_of = model("claude-zephyr-9-20270101");

        assert_eq!(
            sheet.pricing_for(&unheard_of),
            None,
            "a caller that wants to warn about it must be able to tell"
        );
        assert_eq!(
            sheet.pricing_or_fallback(&unheard_of),
            PriceSheet::FALLBACK_PRICING
        );
        assert_eq!(sheet.context_window_for(&unheard_of), 200_000);
        assert_eq!(sheet.display_name_for(&unheard_of), "zephyr-9-20270101");

        let million = entry(
            "claude-zephyr-9-20270101",
            TokenUsage {
                input: 1_000_000,
                ..TokenUsage::ZERO
            },
            None,
        );
        let cost = CostMode::Auto.cost_of(&million, &sheet).dollars();
        assert!(
            (cost - 3.0).abs() < 1e-9,
            "the $3/MTok headline, got {cost}"
        );
    }

    #[test]
    fn an_overriding_row_wins_and_still_matches_before_the_row_it_replaces() {
        // A user correcting Fable 5's input rate must not, as a side effect,
        // move that row past `claude-fable-5-1` -- which is what appending the
        // override would do, and which would then charge every 5.1 response at
        // the 5.0 generation's rates.
        let corrected = PriceSheet::from_rows(
            vec![row(
                "claude-fable-5",
                "Fable 5 (corrected)",
                ModelPricing::from_headline(12.0, 60.0),
            )],
            Provenance::Overridden {
                source: "/home/ada/.config/claude-stats/prices.json".to_owned(),
            },
        );
        let sheet = PriceSheet::builtin().overlaid_with(corrected);

        let fable_5 = sheet
            .pricing_for(&model("claude-fable-5"))
            .expect("the overriding row");
        assert!(
            (fable_5.input.dollars_per_million() - 12.0).abs() < 1e-9,
            "the user's rate wins"
        );
        assert_eq!(
            sheet.display_name_for(&model("claude-fable-5")),
            "Fable 5 (corrected)"
        );

        let point_release = sheet
            .pricing_for(&model("claude-fable-5-1"))
            .expect("still catalogued");
        assert!(
            (point_release.cache_read.dollars_per_million() - 0.25).abs() < 1e-9,
            "5.1 is still reached before the row that would swallow it"
        );

        assert_eq!(
            sheet.rows().count(),
            ModelCatalog::entries().count(),
            "an override of a known model replaces a row rather than adding one"
        );
        assert_eq!(
            sheet.provenance().to_string(),
            "overridden from /home/ada/.config/claude-stats/prices.json"
        );
    }

    #[test]
    fn a_row_for_a_model_the_catalogue_has_never_heard_of_is_placed_where_it_can_match() {
        // `claude-opus-5-turbo` contains `claude-opus-5`, so a row appended to
        // the end of the sheet could never be reached: the substring rule would
        // hand every lookup to the shorter id first. The user would be left
        // with a file that visibly does nothing.
        let addition = PriceSheet::from_rows(
            vec![row(
                "claude-opus-5-turbo",
                "Opus 5 Turbo",
                ModelPricing::from_headline(7.0, 35.0),
            )],
            Provenance::Overridden {
                source: "prices.json".to_owned(),
            },
        );
        let sheet = PriceSheet::builtin().overlaid_with(addition);

        let turbo = sheet
            .pricing_for(&model("claude-opus-5-turbo"))
            .expect("the added row");
        assert!((turbo.input.dollars_per_million() - 7.0).abs() < 1e-9);

        let opus = sheet
            .pricing_for(&model("claude-opus-5"))
            .expect("still catalogued");
        assert!(
            (opus.input.dollars_per_million() - 5.0).abs() < 1e-9,
            "the base model is untouched"
        );
        assert_eq!(sheet.rows().count(), ModelCatalog::entries().count() + 1);
    }

    #[test]
    fn a_full_million_of_every_kind_sums_to_the_published_sheet_row() {
        // One arithmetic anchor per family, asserted against the published
        // rates rather than against whatever the table happens to hold. The
        // five figures are, in order: input, cache read, five-minute write,
        // one-hour write, output. A one-hour write is twice the input rate and
        // a five-minute write is 1.25x it, which is the distinction a single
        // blended "cache write" rate used to lose.
        let sheet = PriceSheet::builtin();
        let published: &[(&str, [f64; 5])] = &[
            ("claude-fable-5", [10.0, 1.0, 12.5, 20.0, 50.0]),
            ("claude-opus-5", [5.0, 0.5, 6.25, 10.0, 25.0]),
            ("claude-sonnet-5", [2.0, 0.2, 2.5, 4.0, 10.0]),
            ("claude-haiku-4-5", [1.0, 0.1, 1.25, 2.0, 5.0]),
        ];

        for (id, [input, read, write_5m, write_1h, output]) in published.iter().copied() {
            let pricing = sheet.pricing_for(&model(id)).expect("a catalogued model");
            let quoted = [
                pricing.input,
                pricing.cache_read,
                pricing.cache_write_5m,
                pricing.cache_write_1h,
                pricing.output,
            ];
            for (quoted, published) in quoted.iter().zip([input, read, write_5m, write_1h, output])
            {
                assert!(
                    (quoted.dollars_per_million() - published).abs() < 1e-9,
                    "{id}: {} is not the published {published}",
                    quoted.dollars_per_million()
                );
            }
            assert!(
                (pricing.cache_write_1h.dollars_per_million()
                    - 2.0 * pricing.input.dollars_per_million())
                .abs()
                    < 1e-9,
                "{id}: a one-hour write is twice the input rate"
            );

            // A million of each kind therefore costs the five rates added up,
            // with no factor of a million left anywhere in the arithmetic.
            let expected = input + read + write_5m + write_1h + output;
            let charged = CostMode::Calculate
                .cost_of(&entry(id, ONE_MILLION_OF_EACH, None), &sheet)
                .dollars();
            assert!(
                (charged - expected).abs() < 1e-9,
                "{id}: charged {charged}, the published row sums to {expected}"
            );
        }
    }

    #[test]
    fn calculate_mode_prices_the_tokens_even_when_a_cost_was_recorded() {
        // The mode to compare two runs with. Its answer must not depend on
        // which entries happened to carry a figure, or a comparison measures
        // the recording rather than the traffic.
        let sheet = PriceSheet::builtin();
        let vouched_for = entry(
            "claude-opus-5",
            TokenUsage {
                input: 1_000_000,
                ..TokenUsage::ZERO
            },
            Some(Usd::new(99.0)),
        );

        let calculated = CostMode::Calculate.cost_of(&vouched_for, &sheet).dollars();
        assert!(
            (calculated - 5.0).abs() < 1e-9,
            "a million Opus 5 input tokens at $5/MTok, got {calculated}"
        );

        // Auto is the mode that defers to the stated figure, which is what
        // makes it the wrong one to compare two runs with.
        assert_eq!(CostMode::Auto.cost_of(&vouched_for, &sheet), Usd::new(99.0));
        assert_eq!(CostMode::default(), CostMode::Auto);
    }

    #[test]
    fn display_mode_reports_nothing_where_nothing_was_recorded_rather_than_inventing_a_figure() {
        // A Claude Code transcript states no per-message cost, so against this
        // format Display reports nothing at all. That is the honest answer to
        // "what is the source willing to vouch for", and it is exactly why it
        // is not the default.
        let sheet = PriceSheet::builtin();
        let unstated = entry(
            "claude-opus-5",
            TokenUsage {
                input: 1_000_000,
                ..TokenUsage::ZERO
            },
            None,
        );

        assert_eq!(CostMode::Display.cost_of(&unstated, &sheet), Usd::ZERO);
        assert!(
            (CostMode::Auto.cost_of(&unstated, &sheet).dollars() - 5.0).abs() < 1e-9,
            "while Auto falls back to pricing the counters"
        );

        let stated = entry("claude-opus-5", TokenUsage::ZERO, Some(Usd::new(1.25)));
        assert_eq!(CostMode::Display.cost_of(&stated, &sheet), Usd::new(1.25));
    }

    #[test]
    fn the_built_in_sheet_says_so_and_an_overridden_one_names_its_file() {
        assert_eq!(Provenance::Builtin.to_string(), "built-in price sheet");
        assert_eq!(
            Provenance::Overridden {
                source: "/etc/prices.json".to_owned(),
            }
            .to_string(),
            "overridden from /etc/prices.json"
        );
    }

    #[test]
    fn overlaying_replaces_only_the_rates_the_overriding_row_states() {
        // A row carries all five rates, so an override that means to change the
        // input rate alone still has to hand over a whole row. What this pins
        // is that the sheet takes the row as given rather than merging it
        // field by field -- the merging happens where the file is read, which
        // is the only place that knows which fields the user actually wrote.
        let mut pricing = ModelPricing::from_headline(5.0, 25.0);
        pricing.cache_read = RatePerMillionTokens::new(0.05);
        let sheet = PriceSheet::builtin().overlaid_with(PriceSheet::from_rows(
            vec![row("claude-opus-5", "Opus 5", pricing)],
            Provenance::Overridden {
                source: "prices.json".to_owned(),
            },
        ));

        let opus = sheet
            .pricing_for(&model("claude-opus-5"))
            .expect("the overriding row");
        assert_eq!(opus, pricing);
        assert!((opus.cache_read.dollars_per_million() - 0.05).abs() < 1e-9);
    }
}
