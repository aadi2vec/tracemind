//! Consent receipts — per-capture audit records.
//!
//! Every ingestion event MUST produce a [`CaptureReceipt`]. Receipts are the
//! foundation of the "librarian, not wire-tap" experience (see
//! `docs/INGESTION_EXPERIENCE_PLAN-2026-07-22.md` §3.4 and §5 S1).
//!
//! The struct is a pure data shape — persistence lives in
//! `tm-ingest::receipts::ReceiptStore`. Keeping it in `tm-types` means every
//! downstream crate can build / inspect receipts without a heavy dep.
//!
//! Modality is stored as a plain string here (rather than an enum) because
//! `tm-types` intentionally does not depend on `tm-ingest` — the multimodal
//! taxonomy lives there.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::tier::MemoryTier;

/// Outcome of the three governance gates applied at capture time.
///
/// `pii_pass` — the first-pass PII scan let this through.
/// `noise_pass` — the noise/length/duplicate filter let it through.
/// `sensitive_app` — true when the source app was on the sensitive
/// blocklist (in that case capture *should* have been rejected; the flag
/// lets an auditor tell "captured despite blocklist" from "captured with
/// clean gates").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct GateDecisions {
    pub pii_pass: bool,
    pub noise_pass: bool,
    pub sensitive_app: bool,
}

impl GateDecisions {
    /// All gates passed and the source app is not on the blocklist.
    pub fn all_clear() -> Self {
        Self {
            pii_pass: true,
            noise_pass: true,
            sensitive_app: false,
        }
    }
}

/// A per-capture audit record.
///
/// Written to `~/.tracemind/receipts.jsonl`. Every field is deliberately
/// stable and JSON-serializable so external auditors (and the user's own
/// Ingestion Review view) can read the log without a TraceMind binary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CaptureReceipt {
    pub capture_id: Uuid,
    pub at: DateTime<Utc>,
    /// e.g. `"clipboard"`, `"screenshot"`, `"web"`, `"mcp"`, `"cli"`.
    pub source: String,
    /// The foreground application at capture time (macOS bundle name / window
    /// title). `None` when unknown (headless / MCP / bench).
    pub app_context: Option<String>,
    /// Stringified `Modality` (kept as `String` to avoid a `tm-ingest` dep).
    pub modality: String,
    pub size_bytes: usize,
    pub gate_decisions: GateDecisions,
    pub tier_on_write: MemoryTier,
    /// The active Focus-mode intent name, if any (see plan §3.1 Mode 2).
    pub user_intent: Option<String>,
    /// Human-readable one-liner explaining why capture happened.
    pub why_captured: String,
}

impl CaptureReceipt {
    /// Build a new receipt with `capture_id = Uuid::new_v4()` and
    /// `at = Utc::now()`.
    pub fn new(
        source: impl Into<String>,
        modality: impl Into<String>,
        size_bytes: usize,
        tier_on_write: MemoryTier,
        why_captured: impl Into<String>,
    ) -> Self {
        Self {
            capture_id: Uuid::new_v4(),
            at: Utc::now(),
            source: source.into(),
            app_context: None,
            modality: modality.into(),
            size_bytes,
            gate_decisions: GateDecisions::all_clear(),
            tier_on_write,
            user_intent: None,
            why_captured: why_captured.into(),
        }
    }

    /// Attach an app context (foreground app name) to this receipt.
    pub fn with_app_context(mut self, app: impl Into<String>) -> Self {
        self.app_context = Some(app.into());
        self
    }

    /// Attach a focus-mode intent to this receipt.
    pub fn with_intent(mut self, intent: impl Into<String>) -> Self {
        self.user_intent = Some(intent.into());
        self
    }

    /// Override the default gate decisions.
    pub fn with_gates(mut self, gates: GateDecisions) -> Self {
        self.gate_decisions = gates;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_clear_gates_have_no_sensitive_app() {
        let g = GateDecisions::all_clear();
        assert!(g.pii_pass);
        assert!(g.noise_pass);
        assert!(!g.sensitive_app);
    }

    #[test]
    fn new_receipt_fields_populated() {
        let r = CaptureReceipt::new(
            "clipboard",
            "text",
            412,
            MemoryTier::Hot,
            "clipboard >30 chars, non-sensitive app",
        );
        assert_eq!(r.source, "clipboard");
        assert_eq!(r.modality, "text");
        assert_eq!(r.size_bytes, 412);
        assert_eq!(r.tier_on_write, MemoryTier::Hot);
        assert!(r.app_context.is_none());
        assert!(r.user_intent.is_none());
        assert_eq!(r.gate_decisions, GateDecisions::all_clear());
    }

    #[test]
    fn builder_helpers_attach_context() {
        let r = CaptureReceipt::new("clipboard", "text", 10, MemoryTier::Cold, "why")
            .with_app_context("Xcode")
            .with_intent("debugging build issue");
        assert_eq!(r.app_context.as_deref(), Some("Xcode"));
        assert_eq!(r.user_intent.as_deref(), Some("debugging build issue"));
    }

    #[test]
    fn json_round_trip() {
        let r = CaptureReceipt::new(
            "clipboard",
            "text",
            412,
            MemoryTier::Hot,
            "clipboard >30 chars",
        )
        .with_app_context("Xcode")
        .with_intent("triage")
        .with_gates(GateDecisions {
            pii_pass: true,
            noise_pass: false,
            sensitive_app: false,
        });

        let encoded = serde_json::to_string(&r).expect("serialize");
        let decoded: CaptureReceipt = serde_json::from_str(&encoded).expect("deserialize");
        assert_eq!(r, decoded);
    }

    #[test]
    fn snake_case_wire_format() {
        let r = CaptureReceipt::new("clipboard", "text", 1, MemoryTier::Hot, "why");
        let v: serde_json::Value = serde_json::to_value(&r).unwrap();
        assert!(v.get("capture_id").is_some(), "capture_id snake_case");
        assert!(v.get("gate_decisions").is_some(), "gate_decisions snake_case");
        assert!(v.get("tier_on_write").is_some(), "tier_on_write snake_case");
        assert!(v.get("why_captured").is_some(), "why_captured snake_case");
        let gates = v.get("gate_decisions").unwrap();
        assert!(gates.get("pii_pass").is_some());
        assert!(gates.get("sensitive_app").is_some());
    }
}
