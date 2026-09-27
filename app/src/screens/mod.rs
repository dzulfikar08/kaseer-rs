//! App screens. Each screen composes themed components; the shell owns
//! screen navigation and the overlay layers.

pub mod campaigns;
pub mod copilot;
pub mod customers;
pub mod dashboard;
pub mod inventory;
pub mod register;
pub mod reports;
pub mod settings;
pub mod shift;

pub use campaigns::CampaignsScreen;
pub use copilot::CopilotScreen;
pub use customers::CustomersScreen;
pub use dashboard::DashboardScreen;
pub use inventory::InventoryScreen;
pub use register::RegisterScreen;
pub use reports::ReportsScreen;
pub use settings::SettingsScreen;
pub use shift::ShiftScreen;
