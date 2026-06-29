// ── src/proactivity.rs : routine mining (Pillar 7) ──────────────────────────
//
// The second brain already records what you do (window focus + clipboard). This
// turns that history into PROACTIVITY: find recurring patterns - the same app
// around the same time on multiple days - so Jarvis can anticipate and offer to
// prepare them. v1 is read-only mining + suggestions (surfaced via `jarvis
// suggest`); a trigger engine that acts on them (with approval) is the next step.
//
// mine_routines is PURE (rows in, routines out) so it is unit-tested without IO.

use chrono::{Datelike, Local, TimeZone, Timelike};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct Routine {
    pub app: String,
    pub hour: u32,
    pub days: usize, // distinct calendar days this (app, hour) was seen
    pub hits: usize, // total occurrences
}

// Find recurring (app, hour-of-day) patterns seen on at least `min_days` distinct
// days, sorted by how habitual they are. Only window-focus rows count.
pub fn mine_routines(rows: &[(i64, String, String, String)], min_days: usize, top: usize) -> Vec<Routine> {
    let mut map: HashMap<(String, u32), (HashSet<i32>, usize)> = HashMap::new();
    for (ts, kind, app, _detail) in rows {
        if kind != "window" {
            continue;
        }
        let app = app.trim();
        if app.is_empty() {
            continue;
        }
        let dt = match Local.timestamp_opt(*ts, 0).single() {
            Some(d) => d,
            None => continue,
        };
        let entry = map.entry((app.to_string(), dt.hour())).or_default();
        entry.0.insert(dt.num_days_from_ce());
        entry.1 += 1;
    }
    let mut routines: Vec<Routine> = map
        .into_iter()
        .filter(|(_, (days, _))| days.len() >= min_days)
        .map(|((app, hour), (days, hits))| Routine { app, hour, days: days.len(), hits })
        .collect();
    routines.sort_by(|a, b| b.days.cmp(&a.days).then(b.hits.cmp(&a.hits)));
    routines.truncate(top);
    routines
}

// Memory consolidation (Pillar 3): collapse raw activity rows into one count per
// (day, app) so old history can be pruned without losing the gist. Pure -> tested.
pub fn summarize_days(rows: &[(i64, String, String, String)]) -> Vec<(String, String, usize)> {
    let mut map: HashMap<(String, String), usize> = HashMap::new();
    for (ts, kind, app, _detail) in rows {
        if kind != "window" {
            continue;
        }
        let app = app.trim();
        if app.is_empty() {
            continue;
        }
        if let Some(dt) = Local.timestamp_opt(*ts, 0).single() {
            *map.entry((dt.format("%Y-%m-%d").to_string(), app.to_string())).or_default() += 1;
        }
    }
    let mut v: Vec<(String, String, usize)> = map.into_iter().map(|((d, a), c)| (d, a, c)).collect();
    v.sort();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarize_groups_by_day_and_app() {
        let base = 1_700_000_000i64;
        let rows = vec![
            (base, "window".to_string(), "Chrome".to_string(), String::new()),
            (base + 60, "window".to_string(), "Chrome".to_string(), String::new()),
            (base + 120, "window".to_string(), "Slack".to_string(), String::new()),
            (base, "clipboard".to_string(), String::new(), "x".to_string()),
        ];
        let s = summarize_days(&rows);
        let chrome = s.iter().find(|(_, a, _)| a == "Chrome").expect("chrome summary");
        assert_eq!(chrome.2, 2); // two Chrome focuses, same day
        assert!(s.iter().any(|(_, a, c)| a == "Slack" && *c == 1));
        assert!(s.iter().all(|(_, a, _)| !a.is_empty())); // clipboard/no-app skipped
    }

    #[test]
    fn mines_recurring_and_skips_oneoffs() {
        let base = 1_700_000_000i64; // a fixed instant
        let day = 86_400i64;
        let rows = vec![
            (base, "window".to_string(), "Chrome".to_string(), "x".to_string()),
            (base + day, "window".to_string(), "Chrome".to_string(), "x".to_string()),
            (base + 2 * day, "window".to_string(), "Chrome".to_string(), "x".to_string()),
            (base + 5, "window".to_string(), "Slack".to_string(), "y".to_string()), // one-off
            (base + 7, "clipboard".to_string(), "".to_string(), "z".to_string()),   // not a window
        ];
        let r = mine_routines(&rows, 2, 5);
        // Chrome recurs across multiple days -> a routine (tz/DST-robust: >= 2 days)
        let chrome = r.iter().find(|x| x.app == "Chrome");
        assert!(chrome.is_some());
        assert!(chrome.unwrap().days >= 2);
        // one-off Slack and non-window rows are not routines
        assert!(r.iter().all(|x| x.app != "Slack"));
        assert!(r.iter().all(|x| !x.app.is_empty()));
    }
}
