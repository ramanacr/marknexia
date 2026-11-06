//! Property tests for the settings document round trip.
//!
//! A small deterministic generator (SplitMix64) drives the cases, so the
//! tests need no extra dependency and every failure reproduces from its
//! case number.

use marknexia_core::settings::{
    AppSettings, CURRENT_SCHEMA_VERSION, ThemePreference, parse_settings, serialize_settings,
};
use marknexia_core::text::eq_ignore_case;

const CASES: u64 = 2_000;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }

    fn bool(&mut self) -> bool {
        self.next() & 1 == 1
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        let index = usize::try_from(self.below(items.len() as u64)).unwrap();
        items[index]
    }

    fn segment(&mut self) -> String {
        const PIECES: &[&str] = &[
            "docs",
            "Read Me",
            "café",
            "ÉCOLE",
            "école",
            "日本語",
            "a.b",
            "x",
            "😀",
            "%20",
            "#",
            "README.md",
            "notes.MD",
            "ß",
            "Ω",
            "\u{a0}",
        ];
        let count = self.below(3) + 1;
        (0..count).map(|_| self.pick(PIECES)).collect()
    }

    fn path(&mut self) -> String {
        let core = match self.below(10) {
            0 => format!("\\\\server\\share\\{}", self.segment()),
            1 => self.segment(),                           // relative: dropped
            2 => format!("\\\\?\\C:\\{}", self.segment()), // device: dropped
            3 => String::from("   "),                      // white space: dropped
            4 => format!("C:\\{}\u{1}", self.segment()),   // control: dropped
            _ => {
                let drive = self.pick(&["C", "c", "D", "z"]);
                let separator = self.pick(&["\\", "/"]);
                let depth = self.below(3) + 1;
                let mut path = format!("{drive}:");
                for _ in 0..depth {
                    path.push_str(separator);
                    path.push_str(&self.segment());
                }
                path
            }
        };
        let pad = self.pick(&["", " ", "\t", "  \u{3000}"]);
        format!("{pad}{core}{pad}")
    }

    fn paths(&mut self) -> Vec<String> {
        let count = self.below(25);
        let mut paths: Vec<String> = Vec::new();
        for _ in 0..count {
            if !paths.is_empty() && self.below(4) == 0 {
                // A case variant of an earlier entry.
                let earlier =
                    paths[usize::try_from(self.below(paths.len() as u64)).unwrap()].to_uppercase();
                paths.push(earlier);
            } else {
                paths.push(self.path());
            }
        }
        paths
    }

    fn settings(&mut self) -> AppSettings {
        let theme = match self.below(3) {
            0 => ThemePreference::System,
            1 => ThemePreference::Light,
            _ => ThemePreference::Dark,
        };
        let sidebar_width = match self.below(8) {
            0 => f64::NAN,
            1 => f64::INFINITY,
            2 => f64::NEG_INFINITY,
            3 => -0.0,
            _ => (self.next() as f64 / u64::MAX as f64) * 800.0,
        };
        AppSettings {
            theme,
            enforce_repository_sandbox: self.bool(),
            allow_external_links: self.bool(),
            allow_remote_assets: self.bool(),
            is_sidebar_open: self.bool(),
            sidebar_mode: i32::try_from(self.below(7)).unwrap() - 3,
            sidebar_width,
            repository_root: self.bool().then(|| self.path()),
            recent_files: self.paths(),
            recent_folders: self.paths(),
        }
    }
}

#[test]
fn serialized_settings_parse_back_to_the_normalized_value() {
    for case in 0..CASES {
        let mut rng = Rng(case);
        let original = rng.settings();
        let expected = original.clone().normalized();

        let bytes = serialize_settings(&original);
        let parsed = parse_settings(&bytes)
            .unwrap_or_else(|error| panic!("case {case}: {error}\n{original:?}"));

        assert_eq!(parsed.schema_version, CURRENT_SCHEMA_VERSION, "case {case}");
        assert_eq!(parsed.settings, expected, "case {case}");
        assert_eq!(serialize_settings(&parsed.settings), bytes, "case {case}");
    }
}

#[test]
fn normalization_is_idempotent_and_within_bounds() {
    for case in 0..CASES {
        let once = Rng(case).settings().normalized();
        assert_eq!(once.clone().normalized(), once, "case {case}");
        assert!((220.0..=520.0).contains(&once.sidebar_width), "case {case}");
        assert!((0..=1).contains(&once.sidebar_mode), "case {case}");
        assert!(once.recent_files.len() <= 15, "case {case}");
        assert!(once.recent_folders.len() <= 10, "case {case}");
        for list in [&once.recent_files, &once.recent_folders] {
            for (index, path) in list.iter().enumerate() {
                assert_eq!(path.trim(), path, "case {case}");
                assert!(
                    !list[..index]
                        .iter()
                        .any(|earlier| eq_ignore_case(earlier, path)),
                    "case {case}: duplicate {path}"
                );
            }
        }
    }
}

#[test]
fn recent_additions_keep_the_newest_first_within_the_cap() {
    for case in 0..CASES {
        let mut rng = Rng(case ^ 0xA5A5);
        let mut settings = rng.settings().normalized();
        let path = format!("C:\\docs\\{}", rng.segment());
        assert!(settings.add_recent_file(&format!("  {path} ")));
        assert_eq!(settings.recent_files[0], path.trim(), "case {case}");
        assert!(settings.recent_files.len() <= 15, "case {case}");
        assert_eq!(settings.clone().normalized(), settings, "case {case}");
    }
}

#[test]
fn parsing_mutated_documents_never_panics() {
    for case in 0..CASES {
        let mut rng = Rng(case ^ 0x5A5A);
        let mut bytes = serialize_settings(&rng.settings());
        for _ in 0..rng.below(8) + 1 {
            let index = usize::try_from(rng.below(bytes.len() as u64)).unwrap();
            match rng.below(3) {
                0 => bytes[index] = u8::try_from(rng.below(256)).unwrap(),
                1 => {
                    bytes.remove(index);
                }
                _ => bytes.insert(index, *b"{}[]\",:0n".get(index % 9).unwrap()),
            }
            if bytes.is_empty() {
                break;
            }
        }
        if let Ok(parsed) = parse_settings(&bytes) {
            // Whatever parses must survive its own round trip.
            let again = parse_settings(&serialize_settings(&parsed.settings)).unwrap();
            assert_eq!(again.settings, parsed.settings, "case {case}");
        }
    }
}
