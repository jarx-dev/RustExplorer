//! The UI's language: English (a fresh install's) or Polish, picked in Plik ▸ Ustawienia. Every
//! text the window shows is a key in `locales/*.yml`, looked up with `t!` in the language chosen
//! now, so a switch shows at the next frame.
use std::fmt::Display;
use std::sync::atomic::{AtomicU8, Ordering};

rust_i18n::i18n!("locales", fallback = "en");

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Lang {
    En,
    Pl,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::En, Lang::Pl];

    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Pl => "pl",
        }
    }

    pub fn from_code(code: &str) -> Option<Lang> {
        Lang::ALL.into_iter().find(|l| l.code() == code)
    }

    /// The language's own name, as the setting lists it.
    pub fn name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Pl => "Polski",
        }
    }
}

static LANG: AtomicU8 = AtomicU8::new(0);

// Tests run in parallel threads: each sets its own thread's language, and Polish is the one the
// older tests were written in.
#[cfg(test)]
thread_local! {
    static TEST_LANG: std::cell::Cell<Lang> = const { std::cell::Cell::new(Lang::Pl) };
}

#[cfg(not(test))]
pub fn lang() -> Lang {
    match LANG.load(Ordering::Relaxed) {
        1 => Lang::Pl,
        _ => Lang::En,
    }
}

#[cfg(test)]
pub fn lang() -> Lang {
    TEST_LANG.with(|l| l.get())
}

pub fn set_lang(l: Lang) {
    #[cfg(test)]
    TEST_LANG.with(|c| c.set(l));
    LANG.store(l as u8, Ordering::Relaxed);
}

/// The language a start uses, from the stored layout ("uklad"): its `lang=` line; none in a
/// layout stored before there was a choice, which keeps such a user on Polish; English for a
/// fresh install (no layout at all) and for a language this build does not know.
pub fn lang_of_layout(layout: Option<&str>) -> Lang {
    match layout {
        None => Lang::En,
        Some(text) => match text.lines().find_map(|l| l.strip_prefix("lang=")) {
            Some(code) => Lang::from_code(code.trim()).unwrap_or(Lang::En),
            None => Lang::Pl,
        },
    }
}

/// The text of `key` in the chosen language (English when that has none; the key itself when no
/// language has it, which the tests rule out for every key in the sources).
pub fn tr(key: &'static str) -> &'static str {
    tr_in(lang(), key)
}

/// `tr` in a given language: for a text cached under the language it was made in, so a switch
/// between reading the language and the lookup cannot store one under the other.
pub fn tr_in(l: Lang, key: &'static str) -> &'static str {
    lookup(l, key).unwrap_or(key)
}

fn lookup(l: Lang, key: &str) -> Option<&'static str> {
    match _rust_i18n_try_translate(l.code(), key)? {
        std::borrow::Cow::Borrowed(s) => Some(s),
        // the tables are compiled in and lent out, never built
        std::borrow::Cow::Owned(s) => Some(Box::leak(s.into_boxed_str())),
    }
}

/// `text` with each `%{name}` replaced by its value.
pub fn fill(text: &str, args: &[(&str, &dyn Display)]) -> String {
    let names: Vec<&str> = args.iter().map(|a| a.0).collect();
    let values: Vec<String> = args.iter().map(|a| a.1.to_string()).collect();
    rust_i18n::replace_patterns(text, &names, &values)
}

/// The plural form of `n` in the chosen language: English `one`/`other`, Polish `one` (1),
/// `few` (2-4, 22-24…, not 12-14) and `many` (0, 5-21, 25…).
pub fn form(n: u64) -> &'static str {
    match lang() {
        Lang::En if n == 1 => "one",
        Lang::En => "other",
        Lang::Pl if n == 1 => "one",
        Lang::Pl if (2..=4).contains(&(n % 10)) && !(12..=14).contains(&(n % 100)) => "few",
        Lang::Pl => "many",
    }
}

/// `key.<form>` for `n` (see `form`): `plural("drives", 5)` is "dysków" in Polish, "drives" in English.
pub fn plural(key: &'static str, n: u64) -> &'static str {
    lookup(lang(), &format!("{key}.{}", form(n))).unwrap_or(key)
}

/// `t!("key")` is the key's text in the chosen language (`&'static str`); `t!("key", name = value, …)`
/// fills its `%{name}` places (`String`).
#[macro_export]
macro_rules! t {
    ($key:literal) => {
        $crate::i18n::tr($key)
    };
    ($key:literal, $($name:ident = $val:expr),+ $(,)?) => {
        $crate::i18n::fill($crate::i18n::tr($key), &[$((stringify!($name), &$val as &dyn ::std::fmt::Display)),+])
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::Path;

    fn table(l: Lang) -> BTreeMap<String, String> {
        _rust_i18n_backend().messages_for_locale(l.code()).unwrap().into_iter().map(|(k, v)| (k.into_owned(), v.into_owned())).collect()
    }

    /// Only the plural forms a language has may be missing from the other.
    fn base(key: &str, l: Lang) -> Option<&str> {
        let own: &[&str] = if l == Lang::En { &["few", "many"] } else { &["other"] };
        match key.rsplit_once('.') {
            Some((_, f)) if own.contains(&f) => None,
            _ => Some(key),
        }
    }

    fn places(text: &str) -> BTreeSet<&str> {
        text.split("%{").skip(1).filter_map(|s| s.split_once('}')).map(|s| s.0).collect()
    }

    #[test]
    fn every_key_is_in_both_languages_with_the_same_places() {
        let (en, pl) = (table(Lang::En), table(Lang::Pl));
        let need = |of: &BTreeMap<String, String>, l| of.keys().filter_map(|k| base(k, l).map(String::from)).collect::<BTreeSet<_>>();
        let only_en: Vec<_> = need(&en, Lang::Pl).into_iter().filter(|k| !pl.contains_key(k)).collect();
        let only_pl: Vec<_> = need(&pl, Lang::En).into_iter().filter(|k| !en.contains_key(k)).collect();
        assert!(only_en.is_empty() && only_pl.is_empty(), "only in en: {only_en:?}\nonly in pl: {only_pl:?}");
        for (k, v) in &en {
            if let Some(p) = pl.get(k) {
                assert_eq!(places(v), places(p), "{k}: the %{{…}} places differ");
            }
        }
        for (k, v) in en.iter().chain(&pl) {
            assert!(!v.trim().is_empty(), "{k} is empty");
        }
        // a plural key has every form of each language
        for (l, t, forms) in [(Lang::En, &en, &["one", "other"][..]), (Lang::Pl, &pl, &["one", "few", "many"][..])] {
            for k in t.keys().filter_map(|k| k.rsplit_once('.').filter(|(_, f)| ["few", "many", "other"].contains(f)).map(|s| s.0)) {
                for f in forms {
                    assert!(t.contains_key(&format!("{k}.{f}")), "{}: {k}.{f} is missing", l.code());
                }
            }
        }
    }

    /// Every `t!("…")`, `tr("…")` and `plural("…", …)` key in the app's sources is in the tables.
    #[test]
    fn every_key_in_the_sources_is_translated() {
        let (en, pl) = (table(Lang::En), table(Lang::Pl));
        let app = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut dirs = vec![app.join("src")];
        dirs.extend(std::fs::read_dir(app.join("crates")).unwrap().map(|e| e.unwrap().path().join("src")));
        let (mut seen, mut missing) = (0, vec![]);
        while let Some(d) = dirs.pop() {
            for e in std::fs::read_dir(&d).into_iter().flatten().map(|e| e.unwrap().path()) {
                if e.is_dir() {
                    dirs.push(e);
                    continue;
                }
                if e.extension().is_none_or(|x| x != "rs") {
                    continue;
                }
                let src: String = std::fs::read_to_string(&e).unwrap().lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
                // a call's first text (tr_in's after the language), and keys kept as data
                // ("undo.what.copy", "type.pdf"), whose text is looked up later with tr/tr_in
                let calls: [(&str, Option<[&str; 2]>); 6] = [
                    ("t!(\"", None),
                    ("tr(\"", None),
                    ("tr_in(", None),
                    ("plural(\"", Some(["one", "other"])),
                    ("\"undo.what.", None),
                    ("\"type.", None),
                ];
                for (call, forms) in calls {
                    // whole names only (not format!("…"))
                    for (at, _) in src.match_indices(call).filter(|(at, _)| !src[..*at].ends_with(|c: char| c.is_alphanumeric() || c == '_' || c == '\\')) {
                        let rest = match call {
                            // a literal right after the language argument ("tr_in(l, "), not the
                            // function's definition or this test's own list
                            "tr_in(" => match src[at + call.len()..].split_once('"') {
                                Some((before, rest)) if before.ends_with(", ") && before.chars().all(|c| c.is_alphanumeric() || "_:, ".contains(c)) => rest,
                                _ => continue,
                            },
                            c if c.starts_with('"') => &src[at + 1..],
                            _ => &src[at + call.len()..],
                        };
                        let key = rest.split('"').next().unwrap();
                        seen += 1;
                        let keys = match forms {
                            Some(f) => f.iter().map(|f| format!("{key}.{f}")).collect(),
                            None => vec![key.to_string()],
                        };
                        for k in keys {
                            if !en.contains_key(&k) || (forms.is_none() && !pl.contains_key(&k)) {
                                missing.push(format!("{}: {k}", e.file_name().unwrap().to_string_lossy()));
                            }
                        }
                    }
                }
            }
        }
        assert!(seen > 100, "only {seen} keys found: wrong folder?");
        assert!(missing.is_empty(), "keys with no text: {missing:#?}");
    }

    #[test]
    fn a_layout_from_before_the_choice_keeps_polish_and_a_fresh_install_gets_english() {
        assert_eq!(lang_of_layout(None), Lang::En);
        assert_eq!(lang_of_layout(Some("zoom=1\nwidths=90,210,140,80\ntabs=C:\\")), Lang::Pl);
        assert_eq!(lang_of_layout(Some("zoom=1\nlang=en\ntabs=C:\\")), Lang::En);
        assert_eq!(lang_of_layout(Some("zoom=1\nlang=pl")), Lang::Pl);
        assert_eq!(lang_of_layout(Some("lang=xx")), Lang::En);
    }

    #[test]
    fn plurals_in_both_languages() {
        let words = |l| {
            set_lang(l);
            [0, 1, 2, 4, 5, 12, 14, 21, 22, 25, 101, 112].map(|n| plural("folders", n))
        };
        assert_eq!(words(Lang::Pl), ["folderów", "folder", "foldery", "foldery", "folderów", "folderów", "folderów", "folderów", "foldery", "folderów", "folderów", "folderów"]);
        assert_eq!(words(Lang::En), ["folders", "folder", "folders", "folders", "folders", "folders", "folders", "folders", "folders", "folders", "folders", "folders"]);
    }

    #[test]
    fn texts_follow_the_language_and_fill_their_places() {
        set_lang(Lang::En);
        assert_eq!(t!("pc.this"), "This PC");
        assert_eq!(t!("ago.min", m = 40), "40 min ago");
        set_lang(Lang::Pl);
        assert_eq!(t!("pc.this"), "Ten komputer");
        assert_eq!(t!("ago.min", m = 40), "40 min temu");
    }
}
