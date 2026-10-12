//! Viewer values: a number per viewer under a name the streamer picks
//! (`coins`, `points`), kept between runs and streams. Counters hold one
//! number each; these hold one per person, so a step can pay "the viewer"
//! and `{top.coins}` can rank them.
//!
//! Viewers are known by their Twitch login. A value at 0 isn't stored: a
//! viewer who spent everything costs nothing to keep.

use crate::json::Value;
use std::collections::BTreeMap;

/// Different values (`coins`, `xp`...) kept at once.
pub const MAX_VALUES: usize = 20;
/// Viewers one value holds. All of them are saved in one file.
pub const MAX_VIEWERS: usize = 50_000;
pub const MAX_NAME: usize = 32;
/// How many `{top.<value>}` lists.
pub const TOP: usize = 5;

/// What a step does to one viewer's value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Add(i64),
    /// Never below 0: a fine takes what is there.
    Subtract(i64),
    Set(i64),
    /// Subtract only when the viewer has that much (and it is 1 or more).
    Spend(i64),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewerValues {
    /// Value name, then login.
    values: BTreeMap<String, BTreeMap<String, i64>>,
}

/// A value's name as it is stored: letters, digits and `_`, so it can sit
/// in `{viewer.<name>}`.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// A login as typed in chat (`@Ana`) as the key viewers are stored under.
pub fn login_key(typed: &str) -> String {
    typed.trim().trim_start_matches('@').to_lowercase()
}

impl ViewerValues {
    pub fn get(&self, value: &str, login: &str) -> i64 {
        self.values
            .get(value)
            .and_then(|viewers| viewers.get(login))
            .copied()
            .unwrap_or(0)
    }

    /// Apply `change`. Returns the new amount and whether it was applied:
    /// only a `Spend` the viewer can't afford isn't.
    pub fn apply(
        &mut self,
        value: &str,
        login: &str,
        change: Change,
    ) -> Result<(i64, bool), String> {
        let current = self.get(value, login);
        let next = match change {
            Change::Add(by) => current.saturating_add(by),
            Change::Subtract(by) => current.saturating_sub(by).max(0),
            Change::Set(to) => to,
            Change::Spend(cost) if cost < 1 || cost > current => return Ok((current, false)),
            Change::Spend(cost) => current - cost,
        };
        self.put(value, login, next)?;
        Ok((next, true))
    }

    fn put(&mut self, value: &str, login: &str, amount: i64) -> Result<(), String> {
        if amount == 0 {
            if let Some(viewers) = self.values.get_mut(value) {
                viewers.remove(login);
            }
            return Ok(());
        }
        if !self.values.contains_key(value) && self.values.len() >= MAX_VALUES {
            return Err(format!("there are already {MAX_VALUES} viewer values"));
        }
        let viewers = self.values.entry(value.to_string()).or_default();
        if viewers.len() >= MAX_VIEWERS && !viewers.contains_key(login) {
            return Err(format!("{value} already holds {MAX_VIEWERS} viewers"));
        }
        viewers.insert(login.to_string(), amount);
        Ok(())
    }

    /// The viewers with the most, highest first; ties by name.
    pub fn top(&self, value: &str, count: usize) -> Vec<(String, i64)> {
        let mut all: Vec<(&String, &i64)> = self.values.get(value).into_iter().flatten().collect();
        all.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        all.into_iter()
            .take(count)
            .map(|(login, amount)| (login.clone(), *amount))
            .collect()
    }

    /// A viewer's place, 1 for the most; 0 when they hold nothing.
    pub fn rank(&self, value: &str, login: &str) -> usize {
        let mine = self.get(value, login);
        if mine <= 0 {
            return 0;
        }
        let ahead = self
            .values
            .get(value)
            .map_or(0, |viewers| viewers.values().filter(|v| **v > mine).count());
        ahead + 1
    }

    /// Each value's name with how many viewers hold some.
    pub fn names(&self) -> Vec<(String, usize)> {
        self.values
            .iter()
            .filter(|(_, viewers)| !viewers.is_empty())
            .map(|(name, viewers)| (name.clone(), viewers.len()))
            .collect()
    }

    /// Forget a whole value. True when there was one.
    pub fn clear(&mut self, value: &str) -> bool {
        self.values.remove(value).is_some_and(|v| !v.is_empty())
    }

    pub fn to_json(&self) -> Value {
        let amounts = |viewers: &BTreeMap<String, i64>| {
            Value::Obj(
                viewers
                    .iter()
                    .map(|(login, amount)| (login.clone(), Value::Num(*amount as f64)))
                    .collect(),
            )
        };
        Value::Obj(
            self.values
                .iter()
                .filter(|(_, viewers)| !viewers.is_empty())
                .map(|(name, viewers)| (name.clone(), amounts(viewers)))
                .collect(),
        )
    }

    /// Read what `to_json` wrote, dropping anything that doesn't fit.
    pub fn from_json(v: Option<&Value>) -> ViewerValues {
        let Some(Value::Obj(fields)) = v else {
            return ViewerValues::default();
        };
        let mut out = ViewerValues::default();
        for (name, viewers) in fields.iter().filter(|(name, _)| valid_name(name)) {
            let Value::Obj(viewers) = viewers else {
                continue;
            };
            for (login, amount) in viewers {
                if let Some(amount) = amount.as_f64() {
                    let _ = out.put(name, login, amount as i64);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spend_only_takes_what_the_viewer_has() {
        let mut v = ViewerValues::default();
        v.apply("coins", "ana", Change::Add(100)).unwrap();
        assert_eq!(
            v.apply("coins", "ana", Change::Spend(150)).unwrap(),
            (100, false)
        );
        assert_eq!(
            v.apply("coins", "ana", Change::Spend(-5)).unwrap(),
            (100, false)
        );
        assert_eq!(
            v.apply("coins", "ana", Change::Spend(100)).unwrap(),
            (0, true)
        );
        assert_eq!(v.get("coins", "ana"), 0);
        assert!(v.names().is_empty(), "a value at 0 isn't kept");
    }

    #[test]
    fn a_fine_never_leaves_a_viewer_in_debt() {
        let mut v = ViewerValues::default();
        v.apply("coins", "ana", Change::Add(10)).unwrap();
        assert_eq!(
            v.apply("coins", "ana", Change::Subtract(25)).unwrap(),
            (0, true)
        );
    }

    #[test]
    fn the_top_and_ranks_order_by_amount() {
        let mut v = ViewerValues::default();
        for (login, coins) in [("ana", 50), ("bo", 900), ("cy", 50), ("di", 300)] {
            v.apply("coins", login, Change::Set(coins)).unwrap();
        }
        let top: Vec<String> = v.top("coins", 3).into_iter().map(|(l, _)| l).collect();
        assert_eq!(top, ["bo", "di", "ana"]);
        assert_eq!(v.rank("coins", "bo"), 1);
        assert_eq!(v.rank("coins", "cy"), 3, "ties share a place");
        assert_eq!(v.rank("coins", "nobody"), 0);
    }

    #[test]
    fn values_survive_the_file() {
        let mut v = ViewerValues::default();
        v.apply("coins", "ana", Change::Add(7)).unwrap();
        v.apply("xp", "bo", Change::Add(3)).unwrap();
        assert_eq!(ViewerValues::from_json(Some(&v.to_json())), v);
        assert_eq!(login_key(" @Ana "), "ana");
    }
}
