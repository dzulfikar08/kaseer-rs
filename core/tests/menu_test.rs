//! Menu pull proof: the companion payload parses into the kiosk model and
//! one apply converges the whole catalog — prices, knowledge, archive
//! flags, photo paths, tax rates, categories — and flags exactly the photo
//! files that need downloading.

use std::fs;

use kaseer_core::db::open_repo;
use kaseer_core::menu;

const MENU_JSON: &str = r#"{
  "v": 1,
  "rev": 7,
  "taxRates": [{ "id": "ppn11", "name": "PPN 11%", "bps": 1100, "inclusive": true }],
  "categories": ["Coffee", "Food"],
  "products": [
    {
      "id": "p-americano", "sku": "8991002100028", "name": "Americano",
      "category": "Coffee", "priceMinor": 16000, "costMinor": 5000,
      "taxRateId": "ppn11", "trackStock": false, "lowStockAt": 0,
      "archived": false,
      "description": "Double shot of espresso lengthened with hot water.",
      "prepNotes": "Pull 2 shots, add hot water.", "allergens": "none",
      "routeKitchen": false, "imagePath": "images/americano-01.jpg"
    },
    {
      "id": "p-old", "sku": "1000000000001", "name": "Retired Promo",
      "category": "Food", "priceMinor": 9000, "costMinor": 3000,
      "taxRateId": null, "trackStock": true, "lowStockAt": 2,
      "archived": true,
      "description": "", "prepNotes": "", "allergens": "",
      "routeKitchen": true, "imagePath": ""
    }
  ]
}"#;

/// A file-backed ledger so photo path resolution lands in a temp dir
/// instead of the working directory.
fn repo_in(dir: &tempfile::TempDir) -> kaseer_core::repos::Repo {
    open_repo(dir.path().join("ledger.db").to_str().unwrap()).unwrap()
}

#[test]
fn parse_menu_reads_the_companion_payload() {
    let payload = menu::parse_menu(MENU_JSON.as_bytes()).unwrap();
    assert_eq!(payload.v, 1);
    assert_eq!(payload.rev, 7);
    assert_eq!(payload.categories, vec!["Coffee", "Food"]);
    assert_eq!(payload.tax_rates.len(), 1);
    assert_eq!(payload.tax_rates[0].bps, 1100);
    assert!(payload.tax_rates[0].inclusive);

    let americano = &payload.products[0];
    assert_eq!(americano.price_minor, 16000);
    assert_eq!(americano.tax_rate_id.as_deref(), Some("ppn11"));
    assert_eq!(americano.image_path, "images/americano-01.jpg");
    assert!(!americano.archived);

    // Archived rows ride the same payload — the register filters them out
    // at read time, not pull time.
    let retired = &payload.products[1];
    assert!(retired.archived);
    assert!(retired.route_kitchen);
    assert_eq!(retired.tax_rate_id, None);

    assert!(menu::parse_menu(b"not json").is_err());
}

#[test]
fn apply_menu_converges_catalog_and_flags_photo_downloads() {
    let dir = tempfile::tempdir().unwrap();
    let repo = repo_in(&dir);

    // Shared-ledger model: the companion serves the SAME file the kiosk
    // opens, so payload ids match seeded rows. Address the seeded
    // americano by its real id.
    let seeded_americano = repo
        .products(false, None)
        .unwrap()
        .into_iter()
        .find(|p| p.sku == "8991002100028")
        .unwrap();
    let payload_json = MENU_JSON.replace("p-americano", &seeded_americano.id);

    let images_dir = menu::images_dir(repo.db_path());
    assert_eq!(images_dir, dir.path().join("images"));

    // First pull: americano's photo path is new and the file is missing —
    // exactly that file is flagged for download, and nothing else.
    let payload = menu::parse_menu(payload_json.as_bytes()).unwrap();
    let apply = menu::apply_menu(&repo, &payload).unwrap();
    assert_eq!(apply.rev, 7);
    assert_eq!(apply.products, 2);
    assert_eq!(apply.image_files, vec!["americano-01.jpg".to_string()]);

    // The catalog converged: price, knowledge, archive flag, photo path,
    // tax rate, and the companion's category list replaced the seed's.
    let all = repo.products(true, None).unwrap();
    let americano = all.iter().find(|p| p.sku == "8991002100028").unwrap();
    assert_eq!(americano.price_minor, 16000);
    assert_eq!(americano.description, "Double shot of espresso lengthened with hot water.");
    assert_eq!(americano.image_path, "images/americano-01.jpg");
    let retired = all.iter().find(|p| p.sku == "1000000000001").unwrap();
    assert!(retired.archived);
    assert!(repo.tax_rates().unwrap().iter().any(|r| r.id == "ppn11" && r.inclusive));
    assert_eq!(repo.categories().unwrap(), vec!["Coffee".to_string(), "Food".to_string()]);

    // Second pull, same revision content but the photo now exists locally:
    // no downloads flagged.
    fs::create_dir_all(&images_dir).unwrap();
    fs::write(images_dir.join("americano-01.jpg"), b"jpeg-bytes").unwrap();
    let apply = menu::apply_menu(&repo, &payload).unwrap();
    assert!(apply.image_files.is_empty());

    // Third pull moves the photo: the new file is flagged, the old one is
    // not (path-based, not file-based).
    let mut next = payload.clone();
    next.rev = 8;
    next.products[0].image_path = "images/americano-02.jpg".to_string();
    let apply = menu::apply_menu(&repo, &next).unwrap();
    assert_eq!(apply.image_files, vec!["americano-02.jpg".to_string()]);
    assert_eq!(repo.products(true, None).unwrap().iter().find(|p| p.sku == "8991002100028").unwrap().image_path, "images/americano-02.jpg");
}

#[test]
fn image_paths_are_validated_before_they_touch_the_filesystem() {
    assert_eq!(menu::image_file_of("images/abc-123_4.jpg").as_deref(), Some("abc-123_4.jpg"));
    assert_eq!(menu::image_file_of("images/x.PNG").as_deref(), Some("x.PNG"));
    assert_eq!(menu::image_file_of(""), None);
    assert_eq!(menu::image_file_of("photos/x.jpg"), None, "only the images/ prefix resolves");
    assert_eq!(menu::image_file_of("images/"), None);
    assert_eq!(menu::image_file_of("images/../secret.db"), None);
    assert_eq!(menu::image_file_of("images/a/b.jpg"), None, "flat names only");
    assert_eq!(menu::image_file_of("images/a\\b.jpg"), None);
    assert_eq!(menu::image_file_of("images/white space.jpg"), None);

    // Traversal-shaped paths never resolve to a local file.
    assert_eq!(menu::local_image_path("./data/ledger.db", "images/../secret.db"), None);
    let resolved = menu::local_image_path("./data/ledger.db", "images/abc.jpg").unwrap();
    assert_eq!(resolved, std::path::Path::new("./data/images").join("abc.jpg"));
}
