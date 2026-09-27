//! Copilot answer engine (Phase 5): the owner asks in plain language, the
//! engine answers from the local ledger only. No LLM, no network, no
//! guessing: intent matching is keyword-based and every figure in an answer
//! is computed from settled sales, with the exact figures it used listed as
//! citations. Questions outside the supported set say so honestly.
//!
//! Calendar convention matches reports.rs: ISO stamps are sliced on their
//! UTC date, weeks roll Monday, so answers line up with the Reports screen.
//! The `now` parameter keeps the pure function deterministic under test.

use chrono::{DateTime, Datelike, Duration, Utc};

use crate::currency::{currency_by_code, round_half_up_div};
use crate::money::{self, MoneyError};
use crate::receipt::Sale;
use crate::repos::{Repo, RepoError, Settings};
use crate::reports::{day_key, ms_day_key, top_products};

#[derive(Debug, thiserror::Error)]
pub enum CopilotError {
    #[error(transparent)]
    Repo(#[from] RepoError),
    #[error(transparent)]
    Money(#[from] MoneyError),
}

pub type Result<T, E = CopilotError> = std::result::Result<T, E>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopilotAnswer {
    pub answer: String,
    /// The exact figures this answer stands on.
    pub citations: Vec<String>,
    /// False when no supported intent matched; the answer then lists what the copilot can do.
    pub matched: bool,
}

/// Velocity window for days-of-cover in the reorder intent.
const VELOCITY_DAYS: i64 = 28;
const MS_DAY: i64 = 86_400_000;
const WEEKDAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const MONTHS: [&str; 12] = [
    "January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December",
];

pub const SUPPORTED_QUESTIONS: [&str; 6] = [
    "Why are sales down this week?",
    "How were sales this month?",
    "What were my best products last month?",
    "Which customers have not returned?",
    "What should I reorder?",
    "How many points does Budi have?",
];

// ── Calendar helpers (UTC day keys, Monday weeks) ────────────────────────

fn utc_midnight_ms(d: DateTime<Utc>) -> i64 {
    d.date_naive()
        .and_hms_opt(0, 0, 0)
        .map(|t| t.and_utc().timestamp_millis())
        .unwrap_or(0)
}

fn iso(ms_or_dt: impl Into<MsSource>) -> String {
    match ms_or_dt.into() {
        MsSource::Ms(ms) => chrono::DateTime::from_timestamp_millis(ms),
        MsSource::Dt(dt) => Some(dt),
    }
    .unwrap_or_default()
    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

enum MsSource {
    Ms(i64),
    Dt(DateTime<Utc>),
}

impl From<i64> for MsSource {
    fn from(ms: i64) -> Self {
        MsSource::Ms(ms)
    }
}

impl From<DateTime<Utc>> for MsSource {
    fn from(dt: DateTime<Utc>) -> Self {
        MsSource::Dt(dt)
    }
}

/// Monday of the UTC week holding `now`, shifted back `weeks_back` weeks.
fn week_start(now: DateTime<Utc>, weeks_back: i64) -> DateTime<Utc> {
    let midnight_ms = utc_midnight_ms(now);
    let since_monday = now.weekday().num_days_from_monday() as i64;
    DateTime::from_timestamp_millis(midnight_ms - (since_monday + weeks_back * 7) * MS_DAY).unwrap_or(now)
}

/// First day of the UTC calendar month holding `now`, shifted back.
fn month_start(now: DateTime<Utc>, months_back: i64) -> DateTime<Utc> {
    let total = now.year() as i64 * 12 + now.month() as i64 - 1 - months_back;
    let year = total.div_euclid(12);
    let month = total.rem_euclid(12) + 1;
    chrono::NaiveDate::from_ymd_opt(year as i32, month as u32, 1)
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|t| t.and_utc())
        .unwrap_or(now)
}

// ── Sales aggregates ─────────────────────────────────────────────────────

struct Totals {
    count: i64,
    total_minor: i64,
    net_minor: i64,
}

fn totals_of(sales: &[Sale]) -> Totals {
    let mut t = Totals { count: 0, total_minor: 0, net_minor: 0 };
    for s in sales {
        t.count += 1;
        t.total_minor += s.total_minor;
        t.net_minor += s.net_minor;
    }
    t
}

struct DayBucket {
    name: &'static str,
    total_minor: i64,
    count: i64,
}

/// One zero-filled bucket per calendar day in [from, now], ascending; sales outside the range are ignored.
fn day_buckets(sales: &[Sale], from_ms: i64, end_ms: i64) -> Vec<(String, DayBucket)> {
    let mut by_key: std::collections::BTreeMap<String, DayBucket> = std::collections::BTreeMap::new();
    let mut ms = from_ms;
    while ms <= end_ms {
        let name = chrono::DateTime::from_timestamp_millis(ms)
            .map(|d| WEEKDAYS[d.weekday().num_days_from_sunday() as usize])
            .unwrap_or("Unknown");
        by_key.insert(ms_day_key(ms), DayBucket { name, total_minor: 0, count: 0 });
        ms += MS_DAY;
    }
    for s in sales {
        if let Some(bucket) = by_key.get_mut(day_key(&s.at)) {
            bucket.total_minor += s.total_minor;
            bucket.count += 1;
        }
    }
    by_key.into_iter().collect()
}

fn slowest_day(sales: &[Sale], from: DateTime<Utc>, now: DateTime<Utc>) -> Option<(String, &'static str, i64)> {
    let buckets = day_buckets(sales, utc_midnight_ms(from), utc_midnight_ms(now));
    let mut slowest: Option<(String, &'static str, i64)> = None;
    for (key, b) in buckets {
        if slowest.as_ref().is_none_or(|(_, _, total)| b.total_minor < *total) {
            slowest = Some((key, b.name, b.total_minor));
        }
    }
    slowest
}

/// Quantities allow three decimals (weighted goods); scale so the math stays integer.
fn scaled_qty(qty: f64) -> i64 {
    (qty * 1_000.0).round() as i64
}

fn qty_text(scaled: i64) -> String {
    if scaled % 1_000 == 0 {
        (scaled / 1_000).to_string()
    } else {
        format!("{:.3}", scaled as f64 / 1_000.0)
    }
}

fn money_text(minor: i64, currency_code: &str) -> Result<String> {
    Ok(money::format(&money::money(minor, currency_code), currency_by_code(currency_code).ok())?)
}

// ── Intent handlers ──────────────────────────────────────────────────────

struct Draft {
    answer: String,
    citations: Vec<String>,
}

fn summarize_range(repo: &Repo, currency: &str, label: &str, from_iso: &str, to_iso: &str) -> Result<Draft> {
    let totals = totals_of(&repo.sales_between(from_iso, to_iso)?);
    let total_text = money_text(totals.total_minor, currency)?;
    if totals.count == 0 {
        let zero = money_text(0, currency)?;
        return Ok(Draft {
            answer: format!("Sales {label}: {zero} across 0 sales. Nothing settled in that range yet."),
            citations: vec![format!("{label}: {zero} across 0 sales")],
        });
    }
    Ok(Draft {
        answer: format!(
            "Sales {label}: {total_text} across {} sales (average basket {}).",
            totals.count,
            money_text(round_half_up_div(totals.total_minor, totals.count), currency)?
        ),
        citations: vec![format!("{label}: {total_text} across {} sales", totals.count)],
    })
}

fn why_sales_down(repo: &Repo, currency: &str, now: DateTime<Utc>) -> Result<Draft> {
    let monday = week_start(now, 0);
    let this_week_sales = repo.sales_between(&iso(monday), &iso(now))?;
    let mine = totals_of(&this_week_sales);
    let theirs = totals_of(&repo.sales_between(&iso(monday - Duration::days(7)), &iso(monday - Duration::days(1)))?);
    let mut citations = vec![
        format!("this week: {} across {} sales", money_text(mine.total_minor, currency)?, mine.count),
        format!("last week: {} across {} sales", money_text(theirs.total_minor, currency)?, theirs.count),
    ];
    if mine.count == 0 && theirs.count == 0 {
        return Ok(Draft {
            answer: "No settled sales this week or last week, so there is nothing to compare yet.".into(),
            citations,
        });
    }
    let slowest = slowest_day(&this_week_sales, monday, now);
    let slowest_note = match &slowest {
        Some((_, name, total)) => {
            citations.push(format!("slowest day {name}: {}", money_text(*total, currency)?));
            format!(" Slowest day so far: {name} at {}.", money_text(*total, currency)?)
        }
        None => String::new(),
    };
    if mine.total_minor == theirs.total_minor {
        return Ok(Draft {
            answer: format!(
                "Sales are level: this week {} across {} sales, same as last week.{slowest_note}",
                money_text(mine.total_minor, currency)?,
                mine.count
            ),
            citations,
        });
    }
    if mine.total_minor > theirs.total_minor {
        let up = mine.total_minor - theirs.total_minor;
        let pct = if theirs.total_minor == 0 {
            String::new()
        } else {
            format!(" (+{}%)", round_half_up_div(up * 100, theirs.total_minor))
        };
        return Ok(Draft {
            answer: format!(
                "Sales are not down: this week is {} across {} sales vs {} across {} sales last week, up {}{pct}.{slowest_note}",
                money_text(mine.total_minor, currency)?,
                mine.count,
                money_text(theirs.total_minor, currency)?,
                theirs.count,
                money_text(up, currency)?
            ),
            citations,
        });
    }
    let down = theirs.total_minor - mine.total_minor;
    let pct = if theirs.total_minor == 0 { 100 } else { round_half_up_div(down * 100, theirs.total_minor) };
    Ok(Draft {
        answer: format!(
            "This week is {} across {} sales vs {} across {} sales last week: down {} ({pct}%).{slowest_note}",
            money_text(mine.total_minor, currency)?,
            mine.count,
            money_text(theirs.total_minor, currency)?,
            theirs.count,
            money_text(down, currency)?
        ),
        citations,
    })
}

fn who_has_not_returned(repo: &Repo, lost_after_days: i64, now: DateTime<Utc>) -> Result<Draft> {
    let now_ms = now.timestamp_millis();
    let cutoff_ms = now_ms - lost_after_days * MS_DAY;
    let mut laggards: Vec<(String, i64, String)> = vec![];
    let mut with_history = 0;
    for customer in repo.customers(None)? {
        let sales = repo.sales_by_customer(&customer.id)?;
        if sales.is_empty() {
            continue;
        }
        with_history += 1;
        let mut last_visit_at = "";
        for s in &sales {
            if s.at.as_str() > last_visit_at {
                last_visit_at = &s.at;
            }
        }
        let last_ms = crate::reports::parse_ms(last_visit_at).unwrap_or(0);
        let days = ((now_ms - last_ms) / MS_DAY).max(0);
        if last_ms < cutoff_ms {
            laggards.push((customer.name, days, last_visit_at.get(..10).unwrap_or("").to_string()));
        }
    }
    laggards.sort_by_key(|(_, days, _)| std::cmp::Reverse(*days));
    if with_history == 0 {
        return Ok(Draft {
            answer: "Nobody has purchased yet, so there is no winback list. Once customers buy, quiet ones show up here.".into(),
            citations: vec![],
        });
    }
    if laggards.is_empty() {
        return Ok(Draft {
            answer: format!(
                "No customer has gone quiet past {lost_after_days} days. Everyone with purchase history is still active."
            ),
            citations: vec![],
        });
    }
    let listed = &laggards[..laggards.len().min(5)];
    Ok(Draft {
        answer: format!(
            "{} customer{} not been back in {lost_after_days}+ days: {}. A WhatsApp nudge with a small offer is the cheapest first move.",
            laggards.len(),
            if laggards.len() == 1 { " has" } else { "s have" },
            listed
                .iter()
                .map(|(name, days, _)| format!("{name} ({days} days)"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        citations: listed
            .iter()
            .map(|(name, days, day)| format!("{name}: last visit {day}, {days} days ago"))
            .collect(),
    })
}

fn what_to_reorder(repo: &Repo, _currency: &str, now: DateTime<Utc>) -> Result<Draft> {
    let mut sold_scaled_by_product: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    for sale in repo.sales_between(&iso(now.timestamp_millis() - VELOCITY_DAYS * MS_DAY), &iso(now))? {
        for line in &sale.lines {
            *sold_scaled_by_product.entry(line.product_id.clone()).or_insert(0) += scaled_qty(line.qty);
        }
    }
    let mut rows: Vec<(String, i64, i64, i64)> = vec![]; // name, on_hand, sold_scaled, days_of_cover
    for product in repo.products(false, None)? {
        if !product.track_stock {
            continue;
        }
        let sold_scaled = sold_scaled_by_product.get(&product.id).copied().unwrap_or(0);
        if sold_scaled <= 0 {
            continue;
        }
        let on_hand = repo.stock_on_hand(&product.id)?.max(0);
        let days_of_cover = (on_hand * VELOCITY_DAYS * 1_000) / sold_scaled;
        rows.push((product.name, on_hand, sold_scaled, days_of_cover));
    }
    rows.sort_by(|a, b| a.3.cmp(&b.3).then(b.2.cmp(&a.2)));
    if rows.is_empty() {
        return Ok(Draft {
            answer: format!(
                "No tracked product has sold in the last {VELOCITY_DAYS} days, so days of cover cannot be computed yet. Reorder answers need recent sales velocity."
            ),
            citations: vec![],
        });
    }
    let listed = &rows[..rows.len().min(5)];
    Ok(Draft {
        answer: format!(
            "Restock first, thinnest cover at the last-{VELOCITY_DAYS}-day pace: {}.",
            listed
                .iter()
                .map(|(name, on_hand, sold, cover)| format!("{name} ({cover} days of cover: {on_hand} on hand, {} sold in {VELOCITY_DAYS} days)", qty_text(*sold)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        citations: listed
            .iter()
            .map(|(name, on_hand, sold, _)| format!("{name}: {on_hand} on hand, {} sold last {VELOCITY_DAYS} days", qty_text(*sold)))
            .collect(),
    })
}

fn best_products(repo: &Repo, currency: &str, now: DateTime<Utc>, this_month: bool) -> Result<Draft> {
    let start = month_start(now, i64::from(!this_month));
    let end = if this_month { now } else { month_start(now, 0) - Duration::days(1) };
    let label = if this_month { "this month" } else { "last month" };
    let month_index = start.month0() as usize;
    let range_name = format!("{} {}", MONTHS[month_index], start.year());
    let range = repo.sales_between(&iso(start), &iso(end))?;
    let totals = totals_of(&range);
    let top = top_products(&range, 5);
    if top.is_empty() {
        return Ok(Draft {
            answer: format!("No settled sales {label} ({range_name}), so there is nothing to rank."),
            citations: vec![],
        });
    }
    let mut ranked_parts: Vec<String> = vec![];
    for (i, p) in top.iter().enumerate() {
        ranked_parts.push(format!(
            "{}. {}, {} sold, {}",
            i + 1,
            p.name,
            qty_text(scaled_qty(p.qty)),
            money_text(p.net_minor, currency)?
        ));
    }
    let ranked = ranked_parts.join(". ");
    let mut citations = vec![format!(
        "{label}: {} net across {} sales",
        money_text(totals.net_minor, currency)?,
        totals.count
    )];
    for (i, p) in top.iter().enumerate() {
        citations.push(format!(
            "{}. {}: {} sold, {}",
            i + 1,
            p.name,
            qty_text(scaled_qty(p.qty)),
            money_text(p.net_minor, currency)?
        ));
    }
    Ok(Draft {
        answer: format!("Best products {label} ({range_name}), by revenue out of {} sales: {ranked}.", totals.count),
        citations,
    })
}

fn points_of(repo: &Repo, currency: &str, customer_id: &str) -> Result<Draft> {
    let Some(customer) = repo.customer_by_id(customer_id)? else {
        return Ok(Draft { answer: "That customer record no longer exists.".into(), citations: vec![] });
    };
    let sales = repo.sales_by_customer(&customer.id)?;
    let lifetime_net_minor: i64 = sales.iter().map(|s| s.net_minor).sum();
    Ok(Draft {
        answer: format!(
            "{} holds {} loyalty points across {} visit{} ({} lifetime net). Points are redeemable at checkout once the customer is attached to the cart.",
            customer.name,
            customer.points,
            sales.len(),
            if sales.len() == 1 { "" } else { "s" },
            money_text(lifetime_net_minor, currency)?
        ),
        citations: vec![format!(
            "{}: {} points, {} visits, {} lifetime net",
            customer.name,
            customer.points,
            sales.len(),
            money_text(lifetime_net_minor, currency)?
        )],
    })
}

fn points_leaders(repo: &Repo) -> Result<Draft> {
    let customers = repo.customers(None)?;
    if customers.is_empty() {
        return Ok(Draft { answer: "No customers exist yet, so there are no points to look up.".into(), citations: vec![] });
    }
    let mut holders: Vec<(String, i64, i64)> = customers
        .iter()
        .map(|c| -> Result<(String, i64, i64)> {
            Ok((c.name.clone(), c.points, repo.sales_by_customer(&c.id)?.len() as i64))
        })
        .collect::<Result<_>>()?;
    holders.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let top: Vec<_> = holders.iter().take(3).collect();
    Ok(Draft {
        answer: format!(
            "Name the customer and I will pull their full record. Top loyalty balances right now: {}.",
            top.iter().map(|(name, points, _)| format!("{name} {points} points")).collect::<Vec<_>>().join(", ")
        ),
        citations: top
            .iter()
            .map(|(name, points, visits)| format!("{name}: {points} points across {visits} visits"))
            .collect(),
    })
}

// ── Intent matching ──────────────────────────────────────────────────────

/// ASCII word-boundary containment, equivalent to the JS oracle's \b...\b
/// on the lowercased question.
fn contains_word(hay: &str, needle: &str) -> bool {
    if needle.is_empty() || hay.is_empty() {
        return false;
    }
    let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut from = 0;
    while let Some(pos) = hay[from..].find(needle) {
        let start = from + pos;
        let end = start + needle.len();
        let before_ok = start == 0 || !is_word(hay.as_bytes()[start - 1]);
        let after_ok = end == hay.len() || !is_word(hay.as_bytes()[end]);
        if before_ok && after_ok {
            return true;
        }
        // Needle is ASCII, so `start + 1` is a char boundary.
        from = start + 1;
    }
    false
}

const DOWN_WORDS: &[&str] = &["down", "dropped", "dropping", "drop", "declined", "decline", "declining", "slower", "weaker", "less than last"];
const SALES_WORDS: &[&str] = &["sales", "revenue", "revenues", "sold", "turnover", "takings"];
const REORDER_PHRASES: &[&str] = &["reorder", "restock", "stock up", "days of cover", "running low", "low stock", "order more"];
const BEST_WORDS: &[&str] = &["best", "top", "popular", "promote", "winner", "hero"];

fn matches_winback(q: &str) -> bool {
    const PLAIN: &[&str] = &[
        "not returned",
        "not return",
        "not coming back",
        "not come back",
        "have not come",
        "has not come",
        "have not been back",
        "has not been back",
        "winback",
        "win-back",
        "lost customer",
        "lapsed",
        "inactive",
        "churn",
        "stopped coming",
        "stopped buying",
        "gone quiet",
        "who has not",
        "who hasn",
    ];
    if PLAIN.iter().any(|p| q.contains(p)) {
        return true;
    }
    // haven.?t (come|been|returned): "haven", one optional any-char, "t", verb.
    let mut from = 0;
    while let Some(pos) = q[from..].find("haven") {
        let rest = &q[from + pos + "haven".len()..];
        for tail in [rest, rest.chars().next().map(|c| &rest[c.len_utf8()..]).unwrap_or("")] {
            if ["t come", "t been", "t returned"].iter().any(|want| tail.starts_with(want)) {
                return true;
            }
        }
        from += pos + 1;
    }
    false
}

/// Answer a question strictly from the local ledger. `now` pins "this week/month" for tests.
pub fn answer(repo: &Repo, question: &str, settings: &Settings, now: DateTime<Utc>) -> Result<CopilotAnswer> {
    let q = question.to_lowercase();
    let currency = settings.currency_code.clone();

    let draft: Draft = if (contains_word(&q, "why") || DOWN_WORDS.iter().any(|w| contains_word(&q, w)))
        && SALES_WORDS.iter().any(|w| contains_word(&q, w))
    {
        why_sales_down(repo, &currency, now)?
    } else if REORDER_PHRASES.iter().any(|p| q.contains(p)) {
        what_to_reorder(repo, &currency, now)?
    } else if contains_word(&q, "points") {
        let customers = repo.customers(None)?;
        let mut best: Option<(&crate::repos::Customer, usize)> = None;
        for c in &customers {
            let name = c.name.to_lowercase().trim().to_string();
            if name.len() >= 2 && q.contains(&name) && best.as_ref().is_none_or(|(_, len)| name.len() > *len) {
                best = Some((c, name.len()));
            }
        }
        match best {
            Some((c, _)) => points_of(repo, &currency, &c.id)?,
            None => points_leaders(repo)?,
        }
    } else if matches_winback(&q) {
        who_has_not_returned(repo, settings.config.lost_after_days, now)?
    } else if BEST_WORDS.iter().any(|w| contains_word(&q, w)) {
        best_products(repo, &currency, now, contains_word(&q, "this month"))?
    } else if SALES_WORDS.iter().any(|w| contains_word(&q, w)) {
        let (from, to, label) = if contains_word(&q, "last week") {
            (week_start(now, 1), week_start(now, 1) + Duration::days(6), "last week")
        } else if contains_word(&q, "this week") {
            (week_start(now, 0), now, "this week")
        } else if contains_word(&q, "last month") {
            (month_start(now, 1), month_start(now, 0) - Duration::days(1), "last month")
        } else {
            (month_start(now, 0), now, "this month")
        };
        summarize_range(repo, &currency, label, &iso(from), &iso(to))?
    } else {
        return Ok(CopilotAnswer {
            answer: format!(
                "I only answer from your local ledger, no guessing. Try one of:\n{}",
                SUPPORTED_QUESTIONS.iter().map(|x| format!("- \"{x}\"")).collect::<Vec<_>>().join("\n")
            ),
            citations: vec![],
            matched: false,
        });
    };
    Ok(CopilotAnswer { answer: draft.answer, citations: draft.citations, matched: true })
}
