//! Where a [`Palette`] is looked up by name.
//!
//! `ThemeRegistry` is a Registry (Fowler, *`PoEAA`*): a well-known object that
//! other code asks "give me the theme named X" rather than each call site
//! carrying its own idea of what "aurora" or "dracula" looks like. Before this
//! module existed the answer to "what colour is CYAN" was "read `theme.rs`";
//! now it is "ask the registry for a palette and read its `accent_primary`
//! field", which is the difference between one place owning the catalogue and
//! every call site owning a fragment of it.

use std::collections::HashMap;
use std::sync::OnceLock;

use super::Palette;
use super::builtins;

/// A catalogue of palettes, keyed by [`Palette::name`].
pub struct ThemeRegistry {
    themes: HashMap<String, Palette>,
}

impl ThemeRegistry {
    /// The registry of every theme shipped with the crate.
    ///
    /// Built once and shared for the life of the process: the twenty-seven
    /// built-in palettes are immutable data, so there is nothing to gain from
    /// rebuilding the same `HashMap` on every lookup, and `OnceLock` is the
    /// standard library's own tool for exactly that -- a value computed once,
    /// on first use, and handed out by shared reference from then on.
    #[must_use]
    pub fn builtin() -> &'static Self {
        static REGISTRY: OnceLock<ThemeRegistry> = OnceLock::new();
        REGISTRY.get_or_init(|| {
            let mut registry = Self {
                themes: HashMap::new(),
            };
            for palette in builtins::all() {
                registry.register(palette);
            }
            registry
        })
    }

    /// The palette registered under `name`, if any.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Palette> {
        self.themes.get(name)
    }

    /// Every registered name, in no particular order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.themes.keys().map(String::as_str)
    }

    /// Adds `palette` to the registry, replacing whatever was previously
    /// registered under the same [`Palette::name`].
    ///
    /// Replacing rather than rejecting a collision is deliberate: a later
    /// epic that lets a user override a built-in theme by registering their
    /// own palette under the same name should not have to reach in and remove
    /// the old one first.
    pub fn register(&mut self, palette: Palette) {
        self.themes.insert(palette.name.clone(), palette);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_registry_holds_exactly_twenty_seven_themes() {
        let names: Vec<&str> = ThemeRegistry::builtin().names().collect();
        assert_eq!(names.len(), 27, "got {names:?}");
    }

    #[test]
    fn aurora_is_always_registered() {
        assert!(ThemeRegistry::builtin().get("aurora").is_some());
    }

    #[test]
    fn an_unknown_name_is_none_rather_than_a_panic() {
        assert!(ThemeRegistry::builtin().get("not-a-real-theme").is_none());
    }

    #[test]
    fn registering_a_palette_under_an_existing_name_replaces_it() {
        let mut registry = ThemeRegistry {
            themes: HashMap::new(),
        };
        let mut first = builtins::all().remove(0);
        first.name = "custom".to_owned();
        let mut second = first.clone();
        second.background = super::super::Rgb(1, 2, 3);
        registry.register(first);
        registry.register(second);

        assert_eq!(
            registry.get("custom").expect("registered").background,
            super::super::Rgb(1, 2, 3)
        );
    }
}
