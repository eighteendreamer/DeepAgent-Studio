//! How a plugin presents itself, resolved from four sources of decreasing
//! specificity.
//!
//! Agent Plugins v1 deliberately says almost nothing about presentation. §5.2
//! gives a plugin a `name` (an identifier, constrained by §5.5 to lowercase
//! kebab-case), an optional `description`, and an optional `author`. There is no
//! display name, no category, no long-form copy. Every client that shows a
//! plugin gallery therefore grows its own richer metadata, and a plugin that
//! travels between clients arrives with some subset of it.
//!
//! So the display layer needs one answer per field, assembled from whatever is
//! present. The sources, most specific first:
//!
//! 1. **Codex `interface`** — an explicit presentation block authored for a
//!    gallery. When the author wrote it, it wins.
//! 2. **The marketplace catalog entry** the plugin was installed from. This is
//!    the copy the user actually read at install time, and it is often better
//!    maintained than the package's own manifest. Concretely, in
//!    `anthropics/claude-code`, `hookify`'s manifest says "Easily create hooks
//!    to prevent unwanted behaviors by analyzing conversation patterns" while
//!    the catalog says "…or from explicit instructions. Define rules via simple
//!    markdown files." The catalog is also the only place a Claude plugin's
//!    `category` and, frequently, its author appear at all.
//! 3. **The portable manifest core** — §5.2's `name`, `description`,
//!    `author.name`. Always available for a conforming plugin.
//! 4. **The plugin directory name** — a last resort for the display name, used
//!    when a package has no usable `name`.
//!
//! # The chain is not uniform across fields
//!
//! Claiming "four levels for everything" would be tidy and wrong. Only the
//! display name has all four, because only it needs a guaranteed value:
//!
//! | Field               | 1 `interface`      | 2 marketplace | 3 portable      | 4 directory |
//! | ------------------- | ------------------ | ------------- | --------------- | ----------- |
//! | `display_name`      | `displayName`      | `displayName` | `name`          | yes         |
//! | `short_description` | `shortDescription` | `description` | `description`   | —           |
//! | `long_description`  | `longDescription`  | `description` | `description`   | —           |
//! | `developer_name`    | `developerName`    | `author.name` | `author.name`   | —           |
//! | `category`          | `category`         | `category`    | — (not in v1)   | —           |
//!
//! Fields other than the display name stay `None` when no source supplies them.
//! Substituting filler text here would make an empty description
//! indistinguishable from a real one, and the UI is the layer that should decide
//! what to render for a genuine absence.
//!
//! # Inputs are borrowed plain strings on purpose
//!
//! Each level is a small struct of `Option<&str>` rather than the concrete
//! `PluginInterface` / `PluginMarketplaceEntry` types. That keeps this module
//! from depending on the shipping manifest and marketplace modules, which are
//! scheduled to move, and it lets a caller feed a level from any shape it
//! happens to hold.

/// Shown when no source supplies a display name at all.
///
/// Reaching this means the package has no `name`, no catalog entry, and no
/// usable directory name — a degenerate case, but the UI still needs a string.
pub const UNNAMED_PLUGIN_DISPLAY_NAME: &str = "Unnamed plugin";

/// Level 1: the Codex `interface` block, authored specifically for display.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InterfaceSource<'a> {
    pub display_name: Option<&'a str>,
    pub short_description: Option<&'a str>,
    pub long_description: Option<&'a str>,
    pub developer_name: Option<&'a str>,
    pub category: Option<&'a str>,
}

/// Level 2: the marketplace catalog entry the plugin was installed from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MarketplaceSource<'a> {
    pub display_name: Option<&'a str>,
    /// One description serves both the short and long forms; a catalog has only
    /// the one field.
    pub description: Option<&'a str>,
    pub author_name: Option<&'a str>,
    pub category: Option<&'a str>,
}

/// Level 3: the portable manifest core (§5.2).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PortableSource<'a> {
    /// §5.2 `name`. An identifier rather than a label, but a readable one.
    pub name: Option<&'a str>,
    pub description: Option<&'a str>,
    pub author_name: Option<&'a str>,
}

/// Every presentation source available for one plugin, in priority order.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PresentationSources<'a> {
    pub interface: InterfaceSource<'a>,
    pub marketplace: MarketplaceSource<'a>,
    pub portable: PortableSource<'a>,
    /// Level 4: the on-disk directory name.
    pub directory_name: Option<&'a str>,
}

/// The resolved presentation for one plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Presentation {
    /// Always populated — see [`UNNAMED_PLUGIN_DISPLAY_NAME`] for the degenerate
    /// case.
    pub display_name: String,
    pub short_description: Option<String>,
    pub long_description: Option<String>,
    pub developer_name: Option<String>,
    pub category: Option<String>,
}

/// Resolves each field from the highest-priority source that supplies it.
///
/// A source counts as supplying a value only when it is present and not
/// whitespace-only: `"description": ""` in a manifest is an absent description,
/// not an empty one, and letting it win would blank out a good value from a
/// lower level.
pub fn resolve(sources: PresentationSources<'_>) -> Presentation {
    let PresentationSources {
        interface,
        marketplace,
        portable,
        directory_name,
    } = sources;

    Presentation {
        display_name: first(&[
            interface.display_name,
            marketplace.display_name,
            portable.name,
            directory_name,
        ])
        .unwrap_or_else(|| UNNAMED_PLUGIN_DISPLAY_NAME.to_string()),
        short_description: first(&[
            interface.short_description,
            marketplace.description,
            portable.description,
        ]),
        long_description: first(&[
            interface.long_description,
            marketplace.description,
            portable.description,
        ]),
        developer_name: first(&[
            interface.developer_name,
            marketplace.author_name,
            portable.author_name,
        ]),
        category: first(&[interface.category, marketplace.category]),
    }
}

/// The first candidate that carries non-blank text, trimmed.
fn first(candidates: &[Option<&str>]) -> Option<String> {
    candidates
        .iter()
        .flatten()
        .map(|value| value.trim())
        .find(|value| !value.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every level populated: level 1 wins each field it defines, and `category`
    /// still comes from level 1 because it defines that too.
    #[test]
    fn interface_outranks_every_other_source() {
        let resolved = resolve(PresentationSources {
            interface: InterfaceSource {
                display_name: Some("Hookify"),
                short_description: Some("Create hooks"),
                long_description: Some("Create hooks from conversation patterns"),
                developer_name: Some("Daisy Hollman"),
                category: Some("productivity"),
            },
            marketplace: MarketplaceSource {
                display_name: Some("hookify (catalog)"),
                description: Some("catalog copy"),
                author_name: Some("Anthropic"),
                category: Some("development"),
            },
            portable: PortableSource {
                name: Some("hookify"),
                description: Some("manifest copy"),
                author_name: Some("manifest author"),
            },
            directory_name: Some("hookify-0.1.0"),
        });

        assert_eq!(resolved.display_name, "Hookify");
        assert_eq!(resolved.short_description.as_deref(), Some("Create hooks"));
        assert_eq!(
            resolved.long_description.as_deref(),
            Some("Create hooks from conversation patterns")
        );
        assert_eq!(resolved.developer_name.as_deref(), Some("Daisy Hollman"));
        assert_eq!(resolved.category.as_deref(), Some("productivity"));
    }

    /// Without an `interface`, the catalog entry supplies everything — including
    /// the category and author, which v1 has no place for.
    #[test]
    fn marketplace_entry_is_the_second_level() {
        let resolved = resolve(PresentationSources {
            interface: InterfaceSource::default(),
            marketplace: MarketplaceSource {
                display_name: Some("Hookify"),
                description: Some("catalog copy"),
                author_name: Some("Daisy Hollman"),
                category: Some("productivity"),
            },
            portable: PortableSource {
                name: Some("hookify"),
                description: Some("manifest copy"),
                author_name: Some("manifest author"),
            },
            directory_name: Some("hookify-0.1.0"),
        });

        assert_eq!(resolved.display_name, "Hookify");
        assert_eq!(resolved.short_description.as_deref(), Some("catalog copy"));
        assert_eq!(resolved.long_description.as_deref(), Some("catalog copy"));
        assert_eq!(resolved.developer_name.as_deref(), Some("Daisy Hollman"));
        assert_eq!(resolved.category.as_deref(), Some("productivity"));
    }

    /// The real reason the catalog outranks the manifest: in
    /// `anthropics/claude-code`, the catalog copy for `hookify` is a superset of
    /// the manifest copy. Preferring the manifest would show the user less than
    /// what they read when installing.
    #[test]
    fn catalog_copy_wins_over_a_staler_manifest_description() {
        const MANIFEST: &str =
            "Easily create hooks to prevent unwanted behaviors by analyzing conversation patterns";
        const CATALOG: &str = "Easily create custom hooks to prevent unwanted behaviors by \
                               analyzing conversation patterns or from explicit instructions. \
                               Define rules via simple markdown files.";

        let resolved = resolve(PresentationSources {
            marketplace: MarketplaceSource {
                description: Some(CATALOG),
                ..MarketplaceSource::default()
            },
            portable: PortableSource {
                name: Some("hookify"),
                description: Some(MANIFEST),
                ..PortableSource::default()
            },
            ..PresentationSources::default()
        });

        assert_eq!(resolved.short_description.as_deref(), Some(CATALOG));
    }

    /// A plugin installed from a local directory has no catalog entry, so the
    /// portable core carries the description and the author.
    #[test]
    fn portable_core_is_the_third_level() {
        let resolved = resolve(PresentationSources {
            portable: PortableSource {
                name: Some("hookify"),
                description: Some("manifest copy"),
                author_name: Some("Daisy Hollman"),
            },
            directory_name: Some("hookify-0.1.0"),
            ..PresentationSources::default()
        });

        assert_eq!(resolved.display_name, "hookify");
        assert_eq!(resolved.short_description.as_deref(), Some("manifest copy"));
        assert_eq!(resolved.long_description.as_deref(), Some("manifest copy"));
        assert_eq!(resolved.developer_name.as_deref(), Some("Daisy Hollman"));
        assert_eq!(
            resolved.category, None,
            "v1 has no category, so nothing can supply it at level 3"
        );
    }

    /// A package with no usable `name` still needs a label.
    #[test]
    fn directory_name_is_the_fourth_level() {
        let resolved = resolve(PresentationSources {
            directory_name: Some("hookify-0.1.0"),
            ..PresentationSources::default()
        });

        assert_eq!(resolved.display_name, "hookify-0.1.0");
        assert_eq!(resolved.short_description, None);
        assert_eq!(resolved.long_description, None);
        assert_eq!(resolved.developer_name, None);
        assert_eq!(resolved.category, None);
    }

    #[test]
    fn with_no_source_at_all_the_display_name_is_still_usable() {
        let resolved = resolve(PresentationSources::default());

        assert_eq!(resolved.display_name, UNNAMED_PLUGIN_DISPLAY_NAME);
    }

    /// Peeling one level at a time must move each field down exactly one step,
    /// which a per-field chain can get wrong in a way single-level tests miss.
    #[test]
    fn peeling_levels_degrades_one_step_at_a_time() {
        let interface = InterfaceSource {
            display_name: Some("L1"),
            ..InterfaceSource::default()
        };
        let marketplace = MarketplaceSource {
            display_name: Some("L2"),
            ..MarketplaceSource::default()
        };
        let portable = PortableSource {
            name: Some("L3"),
            ..PortableSource::default()
        };

        let all = PresentationSources {
            interface,
            marketplace,
            portable,
            directory_name: Some("L4"),
        };
        assert_eq!(resolve(all).display_name, "L1");

        let without_interface = PresentationSources {
            interface: InterfaceSource::default(),
            ..all
        };
        assert_eq!(resolve(without_interface).display_name, "L2");

        let without_marketplace = PresentationSources {
            marketplace: MarketplaceSource::default(),
            ..without_interface
        };
        assert_eq!(resolve(without_marketplace).display_name, "L3");

        let only_directory = PresentationSources {
            portable: PortableSource::default(),
            ..without_marketplace
        };
        assert_eq!(resolve(only_directory).display_name, "L4");
    }

    /// A blank string is an absent value. Letting it win would blank out good
    /// copy from a lower level, which is worse than the metadata being missing.
    #[test]
    fn blank_and_whitespace_only_values_do_not_win() {
        let resolved = resolve(PresentationSources {
            interface: InterfaceSource {
                display_name: Some(""),
                short_description: Some("   "),
                developer_name: Some("\t\n"),
                ..InterfaceSource::default()
            },
            marketplace: MarketplaceSource {
                description: Some("catalog copy"),
                author_name: Some("Daisy Hollman"),
                ..MarketplaceSource::default()
            },
            portable: PortableSource {
                name: Some("hookify"),
                ..PortableSource::default()
            },
            ..PresentationSources::default()
        });

        assert_eq!(resolved.display_name, "hookify");
        assert_eq!(resolved.short_description.as_deref(), Some("catalog copy"));
        assert_eq!(resolved.developer_name.as_deref(), Some("Daisy Hollman"));
    }

    #[test]
    fn surrounding_whitespace_is_trimmed() {
        let resolved = resolve(PresentationSources {
            interface: InterfaceSource {
                display_name: Some("  Hookify  "),
                ..InterfaceSource::default()
            },
            ..PresentationSources::default()
        });

        assert_eq!(resolved.display_name, "Hookify");
    }

    /// The short and long forms fall back independently, so a plugin that only
    /// writes `longDescription` still gets a short one from a lower level rather
    /// than none.
    #[test]
    fn short_and_long_descriptions_fall_back_independently() {
        let resolved = resolve(PresentationSources {
            interface: InterfaceSource {
                long_description: Some("the long form"),
                ..InterfaceSource::default()
            },
            portable: PortableSource {
                description: Some("manifest copy"),
                ..PortableSource::default()
            },
            ..PresentationSources::default()
        });

        assert_eq!(resolved.short_description.as_deref(), Some("manifest copy"));
        assert_eq!(resolved.long_description.as_deref(), Some("the long form"));
    }
}
