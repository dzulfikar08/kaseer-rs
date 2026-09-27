//! Ported from kaseer/src/core/campaigns.test.ts — deterministic templates.
//! The canonical input snapshots the exact copy once; the property tests
//! then pin the contract: no invented digits, no invented claims, store and
//! product names from the ledger only, one to three sentences per body.

mod common;

use kaseer_core::campaigns::{generate_campaigns, CampaignChannel, CampaignDraft, CampaignInput, TopProduct};

fn input() -> CampaignInput {
    CampaignInput {
        store_name: "Toko Kaseer".into(),
        new_count: 2,
        loyal_count: 3,
        lost_count: 1,
        high_value_count: 2,
        top_products: vec![TopProduct { name: "Kopi Susu".into(), qty: 120 }, TopProduct { name: "Croissant".into(), qty: 80 }],
        currency_symbol: "Rp".into(),
    }
}

fn draft(channel: CampaignChannel, title: &str, body: &str) -> CampaignDraft {
    CampaignDraft { channel, title: title.to_string(), body: body.to_string() }
}

#[test]
fn same_input_twice_byte_identical_drafts() {
    assert_eq!(generate_campaigns(&input()), generate_campaigns(&input()));
}

#[test]
fn snapshot_exact_copy_for_the_canonical_input() {
    assert_eq!(
        generate_campaigns(&input()),
        vec![
            draft(
                CampaignChannel::Whatsapp,
                "Welcome new faces",
                "Hi {name}, welcome to Toko Kaseer. Next time you pass by, ask for a Kopi Susu and see what the regulars already know.",
            ),
            draft(
                CampaignChannel::Whatsapp,
                "Bring lost customers home",
                "Hi {name}, it has been a while since your last visit to Toko Kaseer. Come back for a Kopi Susu; we kept your spot warm.",
            ),
            draft(
                CampaignChannel::Instagram,
                "Bestsellers post",
                "Counter favorites at Toko Kaseer: Kopi Susu and Croissant. Come see what the fuss is about, then tag us in your photo.",
            ),
            draft(
                CampaignChannel::Promo,
                "Two favorites, one trip",
                "Kopi Susu and Croissant in one basket? That is how a good week starts. Pick both up at Toko Kaseer.",
            ),
            draft(
                CampaignChannel::Promo,
                "Weekend special",
                "Weekends at Toko Kaseer deserve something good. Show this message at the counter for {offer} on Kopi Susu. Every Rp you spend still earns points.",
            ),
            draft(
                CampaignChannel::Loyalty,
                "Loyalty thank you",
                "Dear {name}, thank you for making Toko Kaseer part of your routine. Every visit earns points, and a fresh Kopi Susu is waiting with your name on it.",
            ),
        ]
    );
}

#[test]
fn every_draft_names_the_store_stays_digit_free_and_keeps_one_to_three_sentences() {
    let drafts = generate_campaigns(&input());
    assert!(drafts.len() >= 4);
    for channel in [CampaignChannel::Whatsapp, CampaignChannel::Instagram, CampaignChannel::Promo, CampaignChannel::Loyalty] {
        assert!(drafts.iter().any(|d| d.channel == channel));
    }
    for d in &drafts {
        assert!(d.body.contains("Toko Kaseer"));
        // No fabricated numbers: the templates themselves contain no digits.
        assert!(!d.title.chars().any(|c| c.is_ascii_digit()));
        assert!(!d.body.chars().any(|c| c.is_ascii_digit()));
        assert!(!d.body.contains('—'));
        let sentences = d.body.split(['.', '!', '?']).filter(|s| !s.trim().is_empty()).count();
        assert!((1..=3).contains(&sentences), "sentences out of range in: {}", d.body);
    }
}

#[test]
fn product_mentions_are_real_bestsellers_recipients_stay_placeholders() {
    let bodies: String = generate_campaigns(&input()).into_iter().map(|d| d.body).collect::<Vec<_>>().join("\n");
    assert!(bodies.contains("Kopi Susu"));
    assert!(bodies.contains("Croissant"));
    assert!(bodies.contains("{name}"));
    assert!(bodies.contains("{offer}"));
}

#[test]
fn empty_segments_gate_their_drafts_without_dropping_below_four_total() {
    let mut no_lost = input();
    no_lost.lost_count = 0;
    let no_lost = generate_campaigns(&no_lost);
    assert!(!no_lost.iter().any(|d| d.title == "Bring lost customers home"));
    assert!(no_lost.len() >= 4);

    let mut no_loyalty = input();
    no_loyalty.loyal_count = 0;
    no_loyalty.high_value_count = 0;
    let no_loyalty = generate_campaigns(&no_loyalty);
    assert!(!no_loyalty.iter().any(|d| d.channel == CampaignChannel::Loyalty));
    assert!(no_loyalty.len() >= 4);
}

#[test]
fn no_bestsellers_copy_stays_store_first_with_nothing_invented() {
    let mut no_products = input();
    no_products.top_products = vec![];
    let drafts = generate_campaigns(&no_products);
    assert!(drafts.len() >= 4);
    for d in &drafts {
        assert!(d.body.contains("Toko Kaseer"));
        assert!(!d.body.contains("Kopi Susu"));
        assert!(!d.body.chars().any(|c| c.is_ascii_digit()));
    }
}
