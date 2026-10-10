//! Formatting for the script editor: a world script is shown formatted (line breaks, two-space
//! indentation, Prettier's style) without changing what is stored.
//!
//! JavaScript goes through Biome's formatter (`biome_js_formatter`, MIT OR Apache-2.0), which
//! follows Prettier's defaults: two spaces, semicolons, double quotes unless that needs more
//! escapes, trailing commas, parentheses around arrow parameters, lines up to 100 columns. Lua is
//! not formatted (StyLua would add about 3 MB and the dependencies of its command line).
//!
//! Nothing here runs at start: the formatter's options are built and its code first touched on
//! the first script shown ([`is_loaded`]). Results are cached by source for the session
//! ([`shown_javascript`]). A source that does not parse, is larger than [`MAX_SOURCE`] or takes
//! longer than [`BUDGET`] to format is shown as stored.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Sources longer than this (bytes) are shown as stored.
pub const MAX_SOURCE: usize = 64 * 1024;
/// A format that takes longer than this is dropped and the source shown as stored.
pub const BUDGET: Duration = Duration::from_millis(50);
/// Spaces per indentation level.
pub const INDENT_WIDTH: u8 = 2;
/// The line width the formatter breaks at.
pub const LINE_WIDTH: u16 = 100;
/// The cache is emptied when the sources and results it holds pass this many bytes.
const MAX_CACHE_BYTES: usize = 4 * 1024 * 1024;

/// Why a source is shown as stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Skipped {
    /// This build has no JavaScript formatter (the `javascript` feature is off).
    Unavailable,
    /// Longer than [`MAX_SOURCE`].
    TooLarge,
    /// The source does not parse.
    Syntax,
    /// Formatting took longer than the budget.
    TooSlow,
}

#[cfg(feature = "javascript")]
static OPTIONS: OnceLock<biome_js_formatter::context::JsFormatOptions> = OnceLock::new();

/// Whether the formatter was used in this process (its options built on the first format).
pub fn is_loaded() -> bool {
    #[cfg(feature = "javascript")]
    {
        OPTIONS.get().is_some()
    }
    #[cfg(not(feature = "javascript"))]
    {
        false
    }
}

/// Format a JavaScript world script (not cached). The result ends with a newline only when the
/// source did.
pub fn javascript(source: &str) -> Result<String, Skipped> {
    javascript_within(source, BUDGET)
}

/// [`javascript`] with another time budget.
pub fn javascript_within(source: &str, budget: Duration) -> Result<String, Skipped> {
    if source.len() > MAX_SOURCE {
        return Err(Skipped::TooLarge);
    }
    let started = Instant::now();
    let mut formatted = format_js(source)?;
    if started.elapsed() > budget {
        return Err(Skipped::TooSlow);
    }
    if !source.ends_with('\n') {
        let kept = formatted.trim_end_matches('\n').len();
        formatted.truncate(kept);
    }
    Ok(formatted)
}

#[cfg(feature = "javascript")]
fn format_js(source: &str) -> Result<String, Skipped> {
    use biome_formatter::{IndentStyle, IndentWidth, LineWidth};
    use biome_js_formatter::context::JsFormatOptions;
    use biome_js_parser::JsParserOptions;
    use biome_js_syntax::JsFileSource;

    // World scripts run as classic scripts (not modules).
    let file = JsFileSource::js_script();
    let options = OPTIONS.get_or_init(|| {
        JsFormatOptions::new(file)
            .with_indent_style(IndentStyle::Space)
            .with_indent_width(IndentWidth::from(INDENT_WIDTH))
            .with_line_width(LineWidth::try_from(LINE_WIDTH).unwrap_or_default())
    });
    let parsed = biome_js_parser::parse(source, file, JsParserOptions::default());
    if parsed.has_errors() {
        return Err(Skipped::Syntax);
    }
    let formatted = biome_js_formatter::format_node(options.clone(), &parsed.syntax()).map_err(|_| Skipped::Syntax)?;
    let printed = formatted.print().map_err(|_| Skipped::Syntax)?;
    Ok(printed.into_code())
}

#[cfg(not(feature = "javascript"))]
fn format_js(_source: &str) -> Result<String, Skipped> {
    Err(Skipped::Unavailable)
}

/// Formatted text by source, for the session.
#[derive(Default)]
pub struct Cache {
    entries: HashMap<u64, (Box<str>, Option<Arc<str>>)>,
    bytes: usize,
    runs: u64,
}

impl Cache {
    pub fn new() -> Self {
        Self::default()
    }

    /// The text to show for `source`: its formatted form, or `None` to show it as stored.
    /// Formats once per distinct source.
    pub fn javascript(&mut self, source: &str) -> Option<Arc<str>> {
        let key = {
            let mut hasher = std::hash::DefaultHasher::new();
            source.hash(&mut hasher);
            hasher.finish()
        };
        if let Some((stored, shown)) = self.entries.get(&key)
            && &**stored == source
        {
            return shown.clone();
        }
        self.runs += 1;
        let shown: Option<Arc<str>> = javascript(source).ok().map(Arc::from);
        let size = source.len() + shown.as_ref().map_or(0, |s| s.len());
        if self.bytes + size > MAX_CACHE_BYTES {
            self.entries.clear();
            self.bytes = 0;
        }
        self.bytes += size;
        self.entries.insert(key, (source.into(), shown.clone()));
        shown
    }

    /// How many times this cache ran the formatter.
    pub fn runs(&self) -> u64 {
        self.runs
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

static SHARED: OnceLock<Mutex<Cache>> = OnceLock::new();

/// The text to show for a JavaScript script's `source` (the session's cache), or `None` to show
/// it as stored.
pub fn shown_javascript(source: &str) -> Option<Arc<str>> {
    let cache = SHARED.get_or_init(|| Mutex::new(Cache::new()));
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    cache.javascript(source)
}

/// How many times the session's cache ran the formatter.
pub fn shared_runs() -> u64 {
    SHARED
        .get()
        .map_or(0, |c| c.lock().unwrap_or_else(|e| e.into_inner()).runs())
}

#[cfg(all(test, feature = "javascript"))]
mod tests {
    use super::*;

    #[test]
    fn a_source_that_does_not_parse_is_skipped() {
        let broken = "mud.on(Events.Line, event => {\n    if (event.text.startsWith(\"Exits:\") {\n    }\n});";
        assert_eq!(javascript(broken), Err(Skipped::Syntax));
        let mut cache = Cache::new();
        assert_eq!(cache.javascript(broken), None);
    }

    #[test]
    fn large_and_slow_sources_are_skipped() {
        let large = "mud.echo(\"x\");\n".repeat(MAX_SOURCE / 15 + 1);
        assert!(large.len() > MAX_SOURCE);
        assert_eq!(javascript(&large), Err(Skipped::TooLarge));
        assert_eq!(
            javascript_within("mud.echo( 1 )", Duration::ZERO),
            Err(Skipped::TooSlow)
        );
    }

    #[test]
    fn the_trailing_newline_follows_the_source() {
        assert_eq!(javascript("mud.echo( 1 )").unwrap(), "mud.echo(1);");
        assert_eq!(javascript("mud.echo( 1 )\n").unwrap(), "mud.echo(1);\n");
    }

    #[test]
    fn the_cache_formats_each_source_once() {
        let mut cache = Cache::new();
        let a = cache.javascript("let a=1").unwrap();
        assert_eq!(&*a, "let a = 1;");
        assert_eq!(cache.runs(), 1);
        assert!(Arc::ptr_eq(&a, &cache.javascript("let a=1").unwrap()));
        assert_eq!(cache.runs(), 1);
        assert_eq!(cache.javascript("let b = (").as_deref(), None);
        assert_eq!(cache.javascript("let b = (").as_deref(), None);
        assert_eq!(cache.runs(), 2, "a failure is cached too");
        assert_eq!(cache.len(), 2);
    }
}
