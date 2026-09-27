//! Campaign drafts (Phase 4): deterministic marketing copy grown from the
//! store's own ledger. Same input, same output, so tests can snapshot it
//! and the owner can regenerate without surprises. Copy rules: warm
//! local-shop tone in plain English, every sentence says only what the
//! ledger can prove, no digits the generator invented. Choices that belong
//! to the owner (the offer, the recipients) stay as {offer}/{name}
//! placeholders.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CampaignChannel {
    Whatsapp,
    Instagram,
    Promo,
    Loyalty,
}

impl CampaignChannel {
    pub fn as_str(self) -> &'static str {
        match self {
            CampaignChannel::Whatsapp => "whatsapp",
            CampaignChannel::Instagram => "instagram",
            CampaignChannel::Promo => "promo",
            CampaignChannel::Loyalty => "loyalty",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CampaignDraft {
    pub channel: CampaignChannel,
    pub title: String,
    pub body: String,
}

/// One bestseller row straight from the ledger, busiest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopProduct {
    pub name: String,
    pub qty: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CampaignInput {
    pub store_name: String,
    pub new_count: i64,
    pub loyal_count: i64,
    pub lost_count: i64,
    pub high_value_count: i64,
    /// Best sellers straight from the ledger, busiest first.
    pub top_products: Vec<TopProduct>,
    pub currency_symbol: String,
}

/// Always emits at least four drafts so the studio is never empty while the
/// shop has any data at all. Segment-specific drafts (win back, loyalty)
/// only appear when the ledger actually shows those customers; the rest are
/// product and store drafts that work from day one.
pub fn generate_campaigns(input: &CampaignInput) -> Vec<CampaignDraft> {
    let store = &input.store_name;
    let p1 = input.top_products.first().map(|p| p.name.as_str()).unwrap_or("");
    let p2 = input.top_products.get(1).map(|p| p.name.as_str()).unwrap_or("");
    let mut drafts: Vec<CampaignDraft> = vec![];

    drafts.push(CampaignDraft {
        channel: CampaignChannel::Whatsapp,
        title: "Welcome new faces".into(),
        body: if p1.is_empty() {
            format!("Hi {{name}}, welcome to {store}. We are glad you found us, come by again soon.")
        } else {
            format!("Hi {{name}}, welcome to {store}. Next time you pass by, ask for a {p1} and see what the regulars already know.")
        },
    });

    if input.lost_count > 0 {
        drafts.push(CampaignDraft {
            channel: CampaignChannel::Whatsapp,
            title: "Bring lost customers home".into(),
            body: if p1.is_empty() {
                format!("Hi {{name}}, it has been a while since your last visit to {store}. Come back soon, we kept your spot warm.")
            } else {
                format!("Hi {{name}}, it has been a while since your last visit to {store}. Come back for a {p1}; we kept your spot warm.")
            },
        });
    }

    drafts.push(CampaignDraft {
        channel: CampaignChannel::Instagram,
        title: "Bestsellers post".into(),
        body: if p1.is_empty() {
            format!("Come see what {store} does best, then tag us in your photo.")
        } else {
            format!(
                "Counter favorites at {store}: {p1}{}. Come see what the fuss is about, then tag us in your photo.",
                if p2.is_empty() { String::new() } else { format!(" and {p2}") }
            )
        },
    });

    drafts.push(CampaignDraft {
        channel: CampaignChannel::Promo,
        title: "Two favorites, one trip".into(),
        body: if p1.is_empty() {
            format!("Stock up at {store} this week; your future self says thanks.")
        } else if p2.is_empty() {
            format!("Stock up on {p1} at {store} this week; your future self says thanks.")
        } else {
            format!("{p1} and {p2} in one basket? That is how a good week starts. Pick both up at {store}.")
        },
    });

    drafts.push(CampaignDraft {
        channel: CampaignChannel::Promo,
        title: "Weekend special".into(),
        body: format!(
            "Weekends at {store} deserve something good. Show this message at the counter for {{offer}}{}. Every {} you spend still earns points.",
            if p1.is_empty() { String::new() } else { format!(" on {p1}") },
            input.currency_symbol
        ),
    });

    if input.loyal_count + input.high_value_count > 0 {
        drafts.push(CampaignDraft {
            channel: CampaignChannel::Loyalty,
            title: "Loyalty thank you".into(),
            body: if p1.is_empty() {
                format!("Dear {{name}}, thank you for making {store} part of your routine. Every visit earns points, and yours are waiting at the counter.")
            } else {
                format!("Dear {{name}}, thank you for making {store} part of your routine. Every visit earns points, and a fresh {p1} is waiting with your name on it.")
            },
        });
    }

    drafts
}
